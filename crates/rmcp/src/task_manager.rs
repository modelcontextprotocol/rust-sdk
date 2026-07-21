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
    ) -> Result<serde_json::Value, McpError> {
        let key = key.into();
        let (tx, rx) = oneshot::channel();
        {
            let mut inner = self.inner.lock().expect("task manager lock poisoned");
            let entry = inner.tasks.get_mut(&self.task_id).ok_or_else(|| {
                McpError::internal_error("task no longer exists".to_string(), None)
            })?;
            if !entry.used_input_keys.insert(key.clone()) {
                return Err(McpError::internal_error(
                    format!("inputRequests key {key:?} was already used for this task"),
                    None,
                ));
            }
            entry.pending_inputs.insert(key.clone(), (request, tx));
            entry.touch();
        }
        rx.await.map_err(|_| {
            McpError::internal_error("task cancelled while awaiting input".to_string(), None)
        })
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
}

/// Boxed future representing the async operation backing a task.
pub type TaskFuture = Pin<Box<dyn Future<Output = Result<CallToolResult, McpError>> + Send>>;

struct TaskEntry {
    task: Task,
    /// Terminal payload, if the task has finished.
    terminal: Option<TaskPayload>,
    /// Outstanding input requests keyed by their unique identifier.
    pending_inputs: HashMap<String, (InputRequest, oneshot::Sender<serde_json::Value>)>,
    /// Every key ever used, to enforce uniqueness across the task lifetime.
    used_input_keys: std::collections::HashSet<String>,
    cancel_requested: bool,
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
            pending_inputs: HashMap::new(),
            used_input_keys: std::collections::HashSet::new(),
            cancel_requested: false,
            created: Instant::now(),
            join_handle: None,
        };
        self.inner
            .lock()
            .expect("task manager lock poisoned")
            .tasks
            .insert(task_id.clone(), entry);

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
                        Err(error) => {
                            if entry.cancel_requested {
                                TaskPayload::Cancelled
                            } else {
                                TaskPayload::Failed {
                                    error: error_to_object(&error),
                                }
                            }
                        }
                    });
                    entry.pending_inputs.clear();
                    entry.touch();
                    entry.task.status = entry.current_status();
                }
            }
        });
        if let Some(entry) = self
            .inner
            .lock()
            .expect("task manager lock poisoned")
            .tasks
            .get_mut(&task_id)
        {
            entry.join_handle = Some(handle);
        }
        task
    }

    /// Handle `tasks/get`: return the current [`DetailedTask`] state.
    pub fn get_task(&self, task_id: &str) -> Result<DetailedTask, McpError> {
        let mut inner = self.inner.lock().expect("task manager lock poisoned");
        Self::expire_overdue(&mut inner);
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

    /// Handle `tasks/cancel`: cooperative cancellation.
    ///
    /// Acknowledges immediately and transitions the *observable* task state to
    /// `cancelled` (unless already terminal), but does **not** abort the
    /// underlying future: the operation keeps running so it can perform
    /// cleanup, observing cancellation via
    /// [`TaskContext::is_cancel_requested`] or via the error returned from a
    /// pending [`TaskContext::request_input`] call (whose response channel is
    /// dropped here). Whatever the future eventually produces is discarded —
    /// the terminal `cancelled` state has already been recorded, matching the
    /// spec's eventually-consistent cancellation semantics.
    pub fn cancel_task(&self, task_id: &str) -> Result<(), McpError> {
        let mut inner = self.inner.lock().expect("task manager lock poisoned");
        let entry = inner
            .tasks
            .get_mut(task_id)
            .ok_or_else(|| unknown_task(task_id))?;
        entry.cancel_requested = true;
        if entry.terminal.is_none() {
            entry.terminal = Some(TaskPayload::Cancelled);
            // Dropping the response senders wakes any `request_input` await
            // with an error, giving parked operations a cooperative exit path.
            entry.pending_inputs.clear();
            entry.touch();
            entry.task.status = TaskStatus::Cancelled;
        }
        Ok(())
    }

    /// Number of tasks currently in a non-terminal state.
    pub fn running_task_count(&self) -> usize {
        let inner = self.inner.lock().expect("task manager lock poisoned");
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

    /// Mark tasks whose TTL has elapsed as `failed` (spec: servers MAY fail
    /// tasks any time after TTL expiry).
    fn expire_overdue(inner: &mut TaskManagerInner) {
        for entry in inner.tasks.values_mut() {
            if entry.terminal.is_none()
                && let Some(ttl_ms) = entry.task.ttl_ms
                && entry.created.elapsed().as_millis() > u128::from(ttl_ms)
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
                entry.pending_inputs.clear();
                entry.touch();
                entry.task.status = TaskStatus::Failed;
            }
        }
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
    async fn cancel_marks_task_cancelled() {
        let manager = TaskManager::new();
        let task = manager.spawn(TaskOptions::default(), |_ctx| {
            Box::pin(async {
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                Ok(ok_result("never"))
            })
        });
        manager.cancel_task(&task.task_id).unwrap();
        let detailed = manager.get_task(&task.task_id).unwrap();
        assert_eq!(detailed.status(), TaskStatus::Cancelled);
    }

    #[tokio::test]
    async fn cancel_is_cooperative_and_lets_the_operation_clean_up() {
        let manager = TaskManager::new();
        let (cleanup_tx, cleanup_rx) = oneshot::channel::<&'static str>();
        let task = manager.spawn(TaskOptions::default(), |ctx| {
            Box::pin(async move {
                // Poll cancellation cooperatively, then run cleanup.
                for _ in 0..500 {
                    if ctx.is_cancel_requested() {
                        let _ = cleanup_tx.send("cleaned up");
                        return Ok(ok_result("cancelled cooperatively"));
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
                Ok(ok_result("never cancelled"))
            })
        });

        manager.cancel_task(&task.task_id).unwrap();

        // Observable state is cancelled immediately (ack + tasks/get)...
        let detailed = manager.get_task(&task.task_id).unwrap();
        assert_eq!(detailed.status(), TaskStatus::Cancelled);

        // ...but the operation keeps running and gets to perform cleanup.
        let cleanup = tokio::time::timeout(std::time::Duration::from_secs(5), cleanup_rx)
            .await
            .expect("cleanup should not time out")
            .expect("cleanup channel should not be dropped");
        assert_eq!(cleanup, "cleaned up");

        // The late result is discarded; the terminal state stays cancelled.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let detailed = manager.get_task(&task.task_id).unwrap();
        assert_eq!(detailed.status(), TaskStatus::Cancelled);
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
    async fn unknown_task_is_an_error() {
        let manager = TaskManager::new();
        assert!(manager.get_task("nope").is_err());
        assert!(manager.cancel_task("nope").is_err());
        assert!(manager.update_task("nope", []).is_err());
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
