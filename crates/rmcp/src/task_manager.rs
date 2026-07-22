//! Server-side runtime for the MCP Tasks extension (SEP-2663,
//! `io.modelcontextprotocol/tasks`).
//!
//! [`TaskManager`] owns the durable state for tasks a server has materialized
//! in response to task-eligible requests (currently `tools/call`). It:
//!
//! - spawns the underlying operation and tracks its lifecycle as a
//!   [`DetailedTask`] (`working` → terminal, optionally via `input_required`),
//! - answers `tasks/get` with the current state (including in-flight
//!   `inputRequests` and terminal `result`/`error` payloads),
//! - accepts `tasks/update` `inputResponses` and routes them to the running
//!   operation (ignoring unknown or already-answered keys per spec),
//! - handles cooperative `tasks/cancel`,
//! - enforces TTL-based expiry (`ttl_ms`), marking overdue tasks `failed`.
//!
//! Tasks are only durably observable once [`TaskManager::spawn`] returns,
//! satisfying the spec requirement that a server not return `CreateTaskResult`
//! before `tasks/get` for that id would resolve.

use std::{
    collections::HashMap,
    pin::Pin,
    sync::{Arc, Mutex},
    time::Instant,
};

use futures::Future;
use tokio::sync::oneshot;

use crate::{
    error::ErrorData as McpError,
    model::{
        CallToolResult, DetailedTask, InputRequest, InputRequests, JsonObject, Task, TaskPayload,
        TaskStatus,
    },
};

/// Default TTL (5 minutes, in milliseconds) applied when none is specified.
pub const DEFAULT_TASK_TTL_MS: u64 = 300_000;

/// Default suggested polling interval, in milliseconds.
pub const DEFAULT_POLL_INTERVAL_MS: u64 = 1_000;

/// Helper to generate an ISO 8601 timestamp for task metadata.
pub fn current_timestamp() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Handle passed to a running task operation, allowing it to surface
/// server-to-client requests (elicitation, sampling, roots) mid-task and
/// await the client's `tasks/update` response.
#[derive(Clone)]
pub struct TaskContext {
    task_id: String,
    inner: Arc<Mutex<TaskManagerInner>>,
}

impl TaskContext {
    /// The id of the task this context belongs to.
    pub fn task_id(&self) -> &str {
        &self.task_id
    }

    /// Surface a server-to-client request under `key` and wait for the
    /// client's response delivered via `tasks/update`.
    ///
    /// While at least one request is outstanding the task reports
    /// `input_required` from `tasks/get`, with all outstanding requests in
    /// `inputRequests`. Keys must be unique over the lifetime of the task;
    /// reusing a key returns an error.
    pub async fn request_input(
        &self,
        key: impl Into<String>,
        request: InputRequest,
    ) -> Result<serde_json::Value, TaskExit> {
        let key = key.into();
        let (tx, rx) = oneshot::channel();
        {
            let mut inner = self.inner.lock().expect("task manager lock poisoned");
            let entry = inner.tasks.get_mut(&self.task_id).ok_or_else(|| {
                TaskExit::Error(McpError::internal_error(
                    "task no longer exists".to_string(),
                    None,
                ))
            })?;
            if !entry.used_input_keys.insert(key.clone()) {
                return Err(TaskExit::Error(McpError::internal_error(
                    format!("inputRequests key {key:?} was already used for this task"),
                    None,
                )));
            }
            entry.pending_inputs.insert(key.clone(), (request, tx));
            entry.touch();
        }
        // The sender is dropped when `tasks/cancel` clears pending inputs.
        rx.await.map_err(|_| TaskExit::Cancelled)
    }

    /// Update the task's human-readable status message.
    pub fn set_status_message(&self, message: impl Into<String>) {
        let mut inner = self.inner.lock().expect("task manager lock poisoned");
        if let Some(entry) = inner.tasks.get_mut(&self.task_id) {
            entry.task.status_message = Some(message.into());
            entry.touch();
        }
    }

    /// Returns `true` if `tasks/cancel` has been received for this task.
    /// Cooperative: operations should check this and stop when set.
    pub fn is_cancel_requested(&self) -> bool {
        let inner = self.inner.lock().expect("task manager lock poisoned");
        inner
            .tasks
            .get(&self.task_id)
            .is_some_and(|e| e.cancel_requested)
    }

    /// Resolves once `tasks/cancel` has been received for this task (or
    /// immediately, if it already has). Cooperative: pair with
    /// `tokio::select!` around long-running work to implement a cancellation
    /// exit path.
    ///
    /// An operation that stops in response should return
    /// [`TaskExit::Cancelled`] so the task settles as `cancelled`. Returning
    /// [`TaskExit::Error`] settles as `failed`, and finishing the work
    /// anyway settles as `completed` — per SEP-2663 cancellation is
    /// cooperative and a task may reach a non-`cancelled` terminal status.
    pub async fn cancelled(&self) {
        let mut rx = {
            let inner = self.inner.lock().expect("task manager lock poisoned");
            let Some(entry) = inner.tasks.get(&self.task_id) else {
                return;
            };
            if entry.cancel_requested {
                return;
            }
            entry.cancel_signal.subscribe()
        };
        // Wait until the watch flips to true; a closed channel means the
        // manager dropped the entry, which also unblocks the operation.
        while !*rx.borrow_and_update() {
            if rx.changed().await.is_err() {
                return;
            }
        }
    }
}

/// How a task operation finished without producing a result.
#[expect(
    clippy::exhaustive_enums,
    reason = "error variant for task exit may only be due to error or cancellation"
)]
#[derive(Debug)]
pub enum TaskExit {
    /// The operation is exiting in response to a cancellation request;
    /// the task settles as terminal `cancelled`.
    Cancelled,
    /// A real failure; the task settles as terminal `failed` with the
    /// error inlined, even after `tasks/cancel` was received.
    Error(McpError),
}

impl From<McpError> for TaskExit {
    fn from(error: McpError) -> Self {
        TaskExit::Error(error)
    }
}

/// Boxed future representing the async operation backing a task.
pub type TaskFuture = Pin<Box<dyn Future<Output = Result<CallToolResult, TaskExit>> + Send>>;

struct TaskEntry {
    task: Task,
    /// Terminal payload, if the task has finished.
    terminal: Option<TaskPayload>,
    /// When the task reached its terminal state; drives retention eviction.
    terminal_at: Option<Instant>,
    /// Outstanding input requests keyed by their unique identifier.
    pending_inputs: HashMap<String, (InputRequest, oneshot::Sender<serde_json::Value>)>,
    /// Every key ever used, to enforce uniqueness across the task lifetime.
    used_input_keys: std::collections::HashSet<String>,
    cancel_requested: bool,
    /// Signals the running operation that cancellation was requested
    /// (`true` once `tasks/cancel` arrives). Cooperative: the operation
    /// decides whether and how to stop.
    cancel_signal: tokio::sync::watch::Sender<bool>,
    created: Instant,
    join_handle: Option<tokio::task::JoinHandle<()>>,
}

impl TaskEntry {
    fn touch(&mut self) {
        self.task.last_updated_at = current_timestamp();
    }

    fn current_status(&self) -> TaskStatus {
        match &self.terminal {
            Some(payload) => payload.status(),
            None if !self.pending_inputs.is_empty() => TaskStatus::InputRequired,
            None => TaskStatus::Working,
        }
    }

    fn detailed(&self) -> DetailedTask {
        let payload = match &self.terminal {
            Some(p) => p.clone(),
            None if !self.pending_inputs.is_empty() => TaskPayload::InputRequired {
                input_requests: self
                    .pending_inputs
                    .iter()
                    .map(|(k, (req, _))| (k.clone(), req.clone()))
                    .collect::<InputRequests>(),
            },
            None => TaskPayload::Working,
        };
        DetailedTask::new(self.task.clone(), payload)
    }
}

#[derive(Default)]
struct TaskManagerInner {
    tasks: HashMap<String, TaskEntry>,
}

/// Options controlling a spawned task.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct TaskOptions {
    /// TTL in milliseconds; `None` means unlimited retention.
    pub ttl_ms: Option<u64>,
    /// Suggested polling interval in milliseconds.
    pub poll_interval_ms: Option<u64>,
    /// Initial status message.
    pub status_message: Option<String>,
}

impl Default for TaskOptions {
    fn default() -> Self {
        Self {
            ttl_ms: Some(DEFAULT_TASK_TTL_MS),
            poll_interval_ms: Some(DEFAULT_POLL_INTERVAL_MS),
            status_message: None,
        }
    }
}

impl TaskOptions {
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the TTL in milliseconds. `None` means unlimited retention.
    pub fn with_ttl_ms(mut self, ttl_ms: impl Into<Option<u64>>) -> Self {
        self.ttl_ms = ttl_ms.into();
        self
    }

    /// Set the suggested polling interval in milliseconds.
    pub fn with_poll_interval_ms(mut self, poll_interval_ms: u64) -> Self {
        self.poll_interval_ms = Some(poll_interval_ms);
        self
    }

    /// Set the initial status message.
    pub fn with_status_message(mut self, message: impl Into<String>) -> Self {
        self.status_message = Some(message.into());
        self
    }
}

/// Server-side task store and executor for the SEP-2663 Tasks extension.
///
/// Cheaply cloneable; all clones share the same state.
///
/// # Retention
///
/// Entries are swept opportunistically on every `spawn` / `get_task` /
/// `update_task` / `cancel_task` / `running_task_count` call: non-terminal
/// tasks whose `ttl_ms` has elapsed are marked `failed` (their operation is
/// aborted), and terminal tasks are evicted after being retained for one
/// further `ttl_ms` window past their terminal transition so pollers can
/// observe the final state.
///
/// Note that the retention window intentionally extends past the
/// creation-based lifetime that `ttl_ms` advertises on the wire: a task that
/// runs to its TTL deadline is marked `failed` around `created + ttl_ms` and
/// stays observable until roughly `created + 2 × ttl_ms`. This is compliant —
/// SEP-2663 lets servers delete expired tasks *at any time* after the TTL,
/// so retaining them longer as an observation grace period is a server-side
/// policy choice, not a wire-contract change. Clients may treat the task as
/// unusable after `createdAt + ttlMs` regardless.
///
/// Tasks with `ttl_ms: None` are retained for the lifetime of the manager
/// (spec: unlimited retention) — bound task creation or call
/// [`Self::shutdown`] yourself if you spawn such tasks in a long-lived
/// server. There is no background sweeper; an idle manager holds its entries
/// until the next call.
#[derive(Clone, Default)]
pub struct TaskManager {
    inner: Arc<Mutex<TaskManagerInner>>,
}

impl TaskManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Spawn an operation as a task and return its seed [`Task`] state for a
    /// `CreateTaskResult`. The task is durably observable via
    /// [`Self::get_task`] before this method returns.
    ///
    /// `make_future` receives a [`TaskContext`] for mid-task input requests,
    /// status messages, and cooperative cancellation checks.
    pub fn spawn<F>(&self, options: TaskOptions, make_future: F) -> Task
    where
        F: FnOnce(TaskContext) -> TaskFuture,
    {
        let task_id = uuid::Uuid::new_v4().to_string();
        let now = current_timestamp();
        let mut task = Task::new(task_id.clone(), TaskStatus::Working, now.clone(), now);
        task.ttl_ms = options.ttl_ms;
        task.poll_interval_ms = options.poll_interval_ms;
        task.status_message = options.status_message;

        let entry = TaskEntry {
            task: task.clone(),
            terminal: None,
            terminal_at: None,
            pending_inputs: HashMap::new(),
            used_input_keys: std::collections::HashSet::new(),
            cancel_requested: false,
            cancel_signal: tokio::sync::watch::channel(false).0,
            created: Instant::now(),
            join_handle: None,
        };
        {
            let mut inner = self.inner.lock().expect("task manager lock poisoned");
            // Opportunistic TTL sweep on every task creation, so terminal
            // entries are evicted even if clients never poll again.
            Self::sweep_expired(&mut inner);
            inner.tasks.insert(task_id.clone(), entry);
        }

        let context = TaskContext {
            task_id: task_id.clone(),
            inner: self.inner.clone(),
        };
        let future = make_future(context);
        let inner = self.inner.clone();
        let id_for_task = task_id.clone();
        let handle = tokio::spawn(async move {
            let result = future.await;
            let mut inner = inner.lock().expect("task manager lock poisoned");
            if let Some(entry) = inner.tasks.get_mut(&id_for_task) {
                if entry.terminal.is_none() {
                    entry.terminal = Some(match result {
                        Ok(result) => TaskPayload::Completed {
                            result: result_to_object(&result),
                        },
                        Err(TaskExit::Cancelled) => TaskPayload::Cancelled,
                        Err(TaskExit::Error(error)) => TaskPayload::Failed {
                            error: error_to_object(&error),
                        },
                    });
                    entry.terminal_at = Some(Instant::now());
                    entry.pending_inputs.clear();
                    entry.touch();
                    entry.task.status = entry.current_status();
                }
                // The operation has finished; drop the JoinHandle so it is
                // not retained for the rest of the retention window.
                entry.join_handle = None;
            }
        });
        match self
            .inner
            .lock()
            .expect("task manager lock poisoned")
            .tasks
            .get_mut(&task_id)
        {
            // Only store the handle while the operation is still running: if
            // it already settled, the completion path above ran first and a
            // stored handle would never be cleared.
            Some(entry) => {
                if entry.terminal.is_none() {
                    entry.join_handle = Some(handle);
                }
            }
            // The entry is gone: shutdown() drained the map between the
            // insert and here. Abort rather than leak a detached operation.
            None => handle.abort(),
        }
        task
    }

    /// Handle `tasks/get`: return the current [`DetailedTask`] state.
    pub fn get_task(&self, task_id: &str) -> Result<DetailedTask, McpError> {
        let mut inner = self.inner.lock().expect("task manager lock poisoned");
        Self::sweep_expired(&mut inner);
        let entry = inner
            .tasks
            .get_mut(task_id)
            .ok_or_else(|| unknown_task(task_id))?;
        entry.task.status = entry.current_status();
        Ok(entry.detailed())
    }

    /// Handle `tasks/update`: deliver `inputResponses` to the running
    /// operation. Unknown, already-answered, or superseded keys are ignored
    /// per spec; a partial set of responses is accepted.
    pub fn update_task(
        &self,
        task_id: &str,
        input_responses: impl IntoIterator<Item = (String, serde_json::Value)>,
    ) -> Result<(), McpError> {
        let mut inner = self.inner.lock().expect("task manager lock poisoned");
        Self::sweep_expired(&mut inner);
        let entry = inner
            .tasks
            .get_mut(task_id)
            .ok_or_else(|| unknown_task(task_id))?;
        for (key, value) in input_responses {
            if let Some((_, tx)) = entry.pending_inputs.remove(&key) {
                // Receiver dropped means the operation moved on; ignore.
                let _ = tx.send(value);
            }
        }
        entry.touch();
        entry.task.status = entry.current_status();
        Ok(())
    }

    /// Handle `tasks/cancel`: cooperative cancellation (SEP-2663).
    ///
    /// Records the cancellation *intent* and acknowledges immediately, but
    /// does **not** abort the underlying future or force a terminal state.
    /// The operation observes cancellation via
    /// [`TaskContext::is_cancel_requested`] / [`TaskContext::cancelled`], or
    /// via the error returned from a pending [`TaskContext::request_input`]
    /// call (whose response channel is dropped here), and decides its own
    /// terminal status:
    ///
    /// - stops with [`TaskExit::Cancelled`] → recorded as `cancelled`,
    /// - stops with [`TaskExit::Error`] → recorded as `failed` with the
    ///   error inlined (a real failure after a cancel request is not masked),
    /// - finishes its work anyway → recorded as `completed` — per the spec,
    ///   "the task may still reach a non-`cancelled` terminal status".
    pub fn cancel_task(&self, task_id: &str) -> Result<(), McpError> {
        let mut inner = self.inner.lock().expect("task manager lock poisoned");
        Self::sweep_expired(&mut inner);
        let entry = inner
            .tasks
            .get_mut(task_id)
            .ok_or_else(|| unknown_task(task_id))?;
        entry.cancel_requested = true;
        let _ = entry.cancel_signal.send(true);
        if entry.terminal.is_none() {
            // Wake any operation parked on `request_input`: dropping the
            // response senders resolves those awaits with an error, giving
            // parked operations a cooperative exit path. The task leaves
            // `input_required` and reports `working` until it settles.
            entry.pending_inputs.clear();
            entry.touch();
            entry.task.status = entry.current_status();
        }
        Ok(())
    }

    /// Number of tasks currently in a non-terminal state.
    pub fn running_task_count(&self) -> usize {
        let mut inner = self.inner.lock().expect("task manager lock poisoned");
        Self::sweep_expired(&mut inner);
        inner
            .tasks
            .values()
            .filter(|e| e.terminal.is_none())
            .count()
    }

    /// Abort all running tasks and clear all task state.
    pub fn shutdown(&self) {
        let mut inner = self.inner.lock().expect("task manager lock poisoned");
        for (_, mut entry) in inner.tasks.drain() {
            if let Some(handle) = entry.join_handle.take() {
                handle.abort();
            }
        }
    }

    /// TTL sweep, run from every `TaskManager` entry point (SEP-2663: servers
    /// MAY mark a task `failed` any time after its TTL elapses, and
    /// subsequently delete it at any time; `ttl_ms: None` means unlimited
    /// retention).
    ///
    /// Two phases per entry:
    /// 1. A non-terminal task whose TTL has elapsed is marked `failed` (its
    ///    operation is aborted — the TTL is the SDK's hard-stop safety valve,
    ///    unlike cooperative `tasks/cancel`).
    /// 2. A *terminal* task is evicted once it has been retained for a full
    ///    TTL window after reaching its terminal state, so well-behaved
    ///    pollers get a chance to observe the final status before late
    ///    `tasks/get` calls start returning `-32602`.
    fn sweep_expired(inner: &mut TaskManagerInner) {
        // Phase 1: fail overdue non-terminal tasks.
        for entry in inner.tasks.values_mut() {
            if entry.terminal.is_none()
                && let Some(ttl_ms) = entry.task.ttl_ms
                && entry.created.elapsed().as_millis() >= u128::from(ttl_ms)
            {
                if let Some(handle) = entry.join_handle.take() {
                    handle.abort();
                }
                entry.terminal = Some(TaskPayload::Failed {
                    error: error_to_object(&McpError::internal_error(
                        "task expired: TTL elapsed before completion".to_string(),
                        None,
                    )),
                });
                entry.terminal_at = Some(Instant::now());
                entry.pending_inputs.clear();
                entry.touch();
                entry.task.status = TaskStatus::Failed;
            }
        }
        // Phase 2: evict terminal tasks whose retention window has passed.
        inner.tasks.retain(|_, entry| {
            let (Some(ttl_ms), Some(terminal_at)) = (entry.task.ttl_ms, entry.terminal_at) else {
                return true;
            };
            terminal_at.elapsed().as_millis() < u128::from(ttl_ms)
        });
    }
}

fn unknown_task(task_id: &str) -> McpError {
    McpError::invalid_params(format!("unknown task: {task_id}"), None)
}

fn result_to_object(result: &CallToolResult) -> JsonObject {
    match serde_json::to_value(result) {
        Ok(serde_json::Value::Object(map)) => map,
        _ => JsonObject::new(),
    }
}

fn error_to_object(error: &McpError) -> JsonObject {
    match serde_json::to_value(error) {
        Ok(serde_json::Value::Object(map)) => map,
        _ => JsonObject::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ContentBlock;

    fn ok_result(text: &str) -> CallToolResult {
        CallToolResult::success(vec![ContentBlock::text(text.to_string())])
    }

    #[tokio::test]
    async fn task_completes_and_result_is_inlined() {
        let manager = TaskManager::new();
        let task = manager.spawn(TaskOptions::default(), |_ctx| {
            Box::pin(async { Ok(ok_result("42")) })
        });
        assert_eq!(task.status, TaskStatus::Working);

        // Durable immediately.
        manager.get_task(&task.task_id).unwrap();

        // Wait for completion.
        for _ in 0..100 {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            let detailed = manager.get_task(&task.task_id).unwrap();
            if detailed.status() == TaskStatus::Completed {
                match detailed.payload {
                    TaskPayload::Completed { result } => {
                        assert!(result.contains_key("content"));
                        return;
                    }
                    other => panic!("unexpected payload: {other:?}"),
                }
            }
        }
        panic!("task did not complete");
    }

    #[tokio::test]
    async fn cancel_settles_to_cancelled_when_operation_honors_it() {
        let manager = TaskManager::new();
        let task = manager.spawn(TaskOptions::default(), |ctx| {
            Box::pin(async move {
                tokio::select! {
                    _ = ctx.cancelled() => Err(TaskExit::Cancelled),
                    _ = tokio::time::sleep(std::time::Duration::from_secs(60)) => {
                        Ok(ok_result("never"))
                    }
                }
            })
        });
        manager.cancel_task(&task.task_id).unwrap();

        // The ack is immediate but the terminal state is set by the
        // operation; poll until it settles.
        for _ in 0..100 {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            let detailed = manager.get_task(&task.task_id).unwrap();
            if detailed.status().is_terminal() {
                assert_eq!(detailed.status(), TaskStatus::Cancelled);
                return;
            }
        }
        panic!("task did not settle after cancel");
    }

    #[tokio::test]
    async fn post_cancel_unrelated_error_settles_as_failed() {
        let manager = TaskManager::new();
        let task = manager.spawn(TaskOptions::default(), |ctx| {
            Box::pin(async move {
                // Fail for an unrelated reason after observing the cancel:
                // must be recorded as `failed`, not masked as `cancelled`.
                ctx.cancelled().await;
                Err(TaskExit::Error(McpError::internal_error(
                    "database write failed",
                    None,
                )))
            })
        });
        manager.cancel_task(&task.task_id).unwrap();

        for _ in 0..100 {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            let detailed = manager.get_task(&task.task_id).unwrap();
            if detailed.status().is_terminal() {
                assert_eq!(detailed.status(), TaskStatus::Failed);
                match detailed.payload {
                    TaskPayload::Failed { error } => {
                        assert!(
                            error.get("message").is_some_and(|m| m
                                .as_str()
                                .is_some_and(|s| s.contains("database write failed"))),
                            "error payload should be preserved: {error:?}"
                        );
                    }
                    other => panic!("unexpected payload: {other:?}"),
                }
                return;
            }
        }
        panic!("task did not settle after cancel");
    }

    #[tokio::test]
    async fn cancel_is_cooperative_and_lets_the_operation_clean_up() {
        let manager = TaskManager::new();
        let (cleanup_tx, cleanup_rx) = oneshot::channel::<&'static str>();
        let task = manager.spawn(TaskOptions::default(), |ctx| {
            Box::pin(async move {
                // Wait for cancellation, then run cleanup and finish the
                // work anyway (spec: a task may still reach a
                // non-`cancelled` terminal status).
                ctx.cancelled().await;
                let _ = cleanup_tx.send("cleaned up");
                Ok(ok_result("finished despite cancel"))
            })
        });

        manager.cancel_task(&task.task_id).unwrap();

        // The ack is immediate and does not force a terminal state.
        let detailed = manager.get_task(&task.task_id).unwrap();
        assert!(
            !detailed.status().is_terminal(),
            "cancel must not force terminal state"
        );

        // The operation observes the cancel and performs cleanup.
        let cleanup = tokio::time::timeout(std::time::Duration::from_secs(5), cleanup_rx)
            .await
            .expect("cleanup should not time out")
            .expect("cleanup channel should not be dropped");
        assert_eq!(cleanup, "cleaned up");

        // The operation chose to complete: the task settles as `completed`.
        for _ in 0..100 {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            let detailed = manager.get_task(&task.task_id).unwrap();
            if detailed.status().is_terminal() {
                assert_eq!(detailed.status(), TaskStatus::Completed);
                return;
            }
        }
        panic!("task did not settle after cancel");
    }

    #[tokio::test]
    async fn cancel_wakes_parked_input_requests() {
        let manager = TaskManager::new();
        let (exit_tx, exit_rx) = oneshot::channel::<&'static str>();
        let task = manager.spawn(TaskOptions::default(), |ctx| {
            Box::pin(async move {
                let request: InputRequest = serde_json::from_value(serde_json::json!({
                    "method": "elicitation/create",
                    "params": {
                        "message": "Waiting forever",
                        "requestedSchema": {"type": "object", "properties": {}}
                    }
                }))
                .map_err(|e| McpError::internal_error(e.to_string(), None))?;
                // Parked on input; cancel must wake this await with an error.
                let err = ctx.request_input("k1", request).await.unwrap_err();
                let _ = exit_tx.send("woken");
                Err(err)
            })
        });

        // Wait until the task is parked on the input request.
        for _ in 0..100 {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            if manager.get_task(&task.task_id).unwrap().status() == TaskStatus::InputRequired {
                break;
            }
        }

        manager.cancel_task(&task.task_id).unwrap();
        let woken = tokio::time::timeout(std::time::Duration::from_secs(5), exit_rx)
            .await
            .expect("parked operation should be woken by cancel")
            .expect("exit channel should not be dropped");
        assert_eq!(woken, "woken");
        assert_eq!(
            manager.get_task(&task.task_id).unwrap().status(),
            TaskStatus::Cancelled
        );
    }

    #[tokio::test]
    async fn unknown_task_is_invalid_params() {
        // SEP-2663 §Protocol Errors: invalid or nonexistent taskId is -32602
        // (Invalid params) — MUST for tasks/get, SHOULD for update/cancel.
        let manager = TaskManager::new();
        for err in [
            manager.get_task("nope").unwrap_err(),
            manager.cancel_task("nope").unwrap_err(),
            manager.update_task("nope", []).unwrap_err(),
        ] {
            assert_eq!(err.code, crate::model::ErrorCode::INVALID_PARAMS);
        }
    }

    #[tokio::test]
    async fn terminal_tasks_are_evicted_after_retention_window() {
        let manager = TaskManager::new();
        let task = manager.spawn(TaskOptions::new().with_ttl_ms(50), |_ctx| {
            Box::pin(async { Ok(ok_result("fast")) })
        });

        // Wait for completion; the terminal state stays observable during
        // the retention window.
        let mut completed = false;
        for _ in 0..100 {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            if manager.get_task(&task.task_id).unwrap().status() == TaskStatus::Completed {
                completed = true;
                break;
            }
        }
        assert!(completed, "task should have completed");

        // After a full TTL window past terminal, the entry is evicted and
        // late polls get -32602.
        tokio::time::sleep(std::time::Duration::from_millis(120)).await;
        let err = manager.get_task(&task.task_id).unwrap_err();
        assert_eq!(err.code, crate::model::ErrorCode::INVALID_PARAMS);
        assert_eq!(manager.running_task_count(), 0);
    }

    #[tokio::test]
    async fn abandoned_tasks_are_swept_by_other_entry_points() {
        // A task nobody ever polls again must still be failed + evicted; the
        // sweep runs from spawn() too, so activity on *other* tasks is enough.
        let manager = TaskManager::new();
        let abandoned = manager.spawn(TaskOptions::new().with_ttl_ms(10), |_ctx| {
            Box::pin(async {
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                Ok(ok_result("never"))
            })
        });

        // Let TTL elapse (fails the task), then a second full window
        // (evicts it), without ever calling get_task on the abandoned id.
        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
        let _ = manager.spawn(TaskOptions::default(), |_ctx| {
            Box::pin(async { Ok(ok_result("other")) })
        });
        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
        let _ = manager.spawn(TaskOptions::default(), |_ctx| {
            Box::pin(async { Ok(ok_result("other2")) })
        });

        let err = manager.get_task(&abandoned.task_id).unwrap_err();
        assert_eq!(err.code, crate::model::ErrorCode::INVALID_PARAMS);
    }

    #[tokio::test]
    async fn running_task_count_sweeps_expired_tasks() {
        let manager = TaskManager::new();
        let _task = manager.spawn(TaskOptions::new().with_ttl_ms(10), |_ctx| {
            Box::pin(async {
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                Ok(ok_result("never"))
            })
        });
        assert_eq!(manager.running_task_count(), 1);
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        // The count itself must sweep: the overdue task is failed, not
        // reported as running.
        assert_eq!(manager.running_task_count(), 0);
    }

    #[tokio::test]
    async fn unlimited_ttl_tasks_are_retained() {
        let manager = TaskManager::new();
        let task = manager.spawn(TaskOptions::new().with_ttl_ms(None), |_ctx| {
            Box::pin(async { Ok(ok_result("kept")) })
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        // Sweeps triggered by other entry points must not evict it.
        let _ = manager.spawn(TaskOptions::default(), |_ctx| {
            Box::pin(async { Ok(ok_result("other")) })
        });
        assert_eq!(
            manager.get_task(&task.task_id).unwrap().status(),
            TaskStatus::Completed
        );
    }

    #[tokio::test]
    async fn ttl_expiry_fails_task() {
        let manager = TaskManager::new();
        let task = manager.spawn(
            TaskOptions {
                ttl_ms: Some(10),
                ..Default::default()
            },
            |_ctx| {
                Box::pin(async {
                    tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                    Ok(ok_result("never"))
                })
            },
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let detailed = manager.get_task(&task.task_id).unwrap();
        assert_eq!(detailed.status(), TaskStatus::Failed);
    }

    #[tokio::test]
    async fn input_required_roundtrip() {
        let manager = TaskManager::new();
        let task = manager.spawn(TaskOptions::default(), |ctx| {
            Box::pin(async move {
                let request: InputRequest = serde_json::from_value(serde_json::json!({
                    "method": "elicitation/create",
                    "params": {
                        "message": "What is your name?",
                        "requestedSchema": {"type": "object", "properties": {}}
                    }
                }))
                .map_err(|e| McpError::internal_error(e.to_string(), None))?;
                let response = ctx.request_input("name-1", request).await?;
                let name = response
                    .get("content")
                    .and_then(|c| c.get("name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                Ok(ok_result(&format!("hello {name}")))
            })
        });

        // Wait for the task to surface the input request.
        let mut saw_input_required = false;
        for _ in 0..100 {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            let detailed = manager.get_task(&task.task_id).unwrap();
            if let TaskPayload::InputRequired { input_requests } = &detailed.payload {
                assert!(input_requests.contains_key("name-1"));
                saw_input_required = true;
                break;
            }
        }
        assert!(saw_input_required, "task never reached input_required");

        // Respond via tasks/update.
        manager
            .update_task(
                &task.task_id,
                [(
                    "name-1".to_string(),
                    serde_json::json!({"action": "accept", "content": {"name": "Ada"}}),
                )],
            )
            .unwrap();

        for _ in 0..100 {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            let detailed = manager.get_task(&task.task_id).unwrap();
            if detailed.status() == TaskStatus::Completed {
                return;
            }
        }
        panic!("task did not complete after input response");
    }
}
