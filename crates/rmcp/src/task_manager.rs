use std::{
    collections::HashMap,
    pin::Pin,
    sync::{Arc, RwLock},
};

use futures::Future;
use tokio::sync::mpsc;

use crate::{error::RmcpError as Error, model::ClientRequest, service::RequestContext, RoleServer};

/// Operation message describing a unit of asynchronous work.
#[derive(Debug, Clone)]
pub struct OperationMessage {
    pub operation_id: String,
    pub name: String,
    pub client_request: ClientRequest,
    pub context: RequestContext<RoleServer>,
    /// Optional timeout seconds override for this operation
    pub timeout_secs: Option<u64>,
}

/// Trait for operation result transport
pub trait OperationResultTransport: Send + Sync + 'static {
    fn operation_id(&self) -> &String;
    fn as_any(&self) -> &dyn std::any::Any;
}

/// Type alias for registered async handlers.
/// A handler receives the OperationMessage and returns a future whose output is a Result<Box<dyn OperationResultTransport>, Error>.
pub type AsyncHandler = Arc<
    dyn Fn(
            OperationMessage,
        )
            -> Pin<Box<dyn Future<Output = Result<Box<dyn OperationResultTransport>, Error>> + Send>>
        + Send
        + Sync,
>;

// ===== Operation Processor =====
pub const DEFAULT_TASK_TIMEOUT_SECS: u64 = 300; // 5 minutes
/// Operation processor that coordinates extractors and handlers
pub struct OperationProcessor {
    /// Registry of named async handlers
    handlers: Arc<RwLock<HashMap<String, AsyncHandler>>>,
    /// Currently running tasks keyed by id
    running_tasks: HashMap<String, RunningTask>,
    /// Completed results waiting to be collected
    completed_results: Vec<TaskResult>,
    task_result_receiver: Option<mpsc::UnboundedReceiver<TaskResult>>,
    task_result_sender: mpsc::UnboundedSender<TaskResult>,
}

struct RunningTask {
    task_handle: tokio::task::JoinHandle<()>,
    started_at: std::time::Instant,
    timeout: Option<u64>,
    operation_message: OperationMessage,
}

pub struct TaskResult {
    pub task_id: String,
    pub operation_message: OperationMessage,
    pub result: Result<Box<dyn OperationResultTransport>, Error>,
}

impl OperationProcessor {
    pub fn new() -> Self {
        let (task_result_sender, task_result_receiver) = mpsc::unbounded_channel();
        Self {
            handlers: Arc::new(RwLock::new(HashMap::new())),
            running_tasks: HashMap::new(),
            completed_results: Vec::new(),
            task_result_receiver: Some(task_result_receiver),
            task_result_sender,
        }
    }

    /// Register a named async handler. Returns Err if name already exists.
    pub fn register_handler(
        &self,
        name: impl Into<String>,
        handler: AsyncHandler,
    ) -> Result<(), Error> {
        let name = name.into();
        let mut map = self.handlers.write().unwrap();
        if map.contains_key(&name) {
            return Err(Error::TaskError(format!(
                "Handler '{}' already registered",
                name
            )));
        }
        map.insert(name, handler);
        Ok(())
    }

    /// Submit an operation for asynchronous execution using handler referenced by metadata key "handler".
    pub fn submit_operation(&mut self, message: OperationMessage) -> Result<(), Error> {
        if self.running_tasks.contains_key(&message.operation_id) {
            return Err(Error::TaskError(format!(
                "Operation with id {} is already running",
                message.operation_id
            )));
        }
        let handler_name = message.name.clone();
        let handler_opt = { self.handlers.read().unwrap().get(&handler_name).cloned() };
        let handler = handler_opt.ok_or_else(|| {
            Error::TaskError(format!("Handler '{}' not registered", handler_name))
        })?;
        self.spawn_async_task(handler, message);
        Ok(())
    }

    fn spawn_async_task(&mut self, handler: AsyncHandler, message: OperationMessage) {
        let task_id = message.operation_id.clone();
        let task_id_for_running = task_id.clone();
        let timeout = message.timeout_secs.or(Some(DEFAULT_TASK_TIMEOUT_SECS));
        let sender = self.task_result_sender.clone();
        let msg_clone = message.clone();
        let handle = tokio::spawn(async move {
            let result = handler(msg_clone.clone()).await;
            let task_result = TaskResult {
                task_id: task_id,
                operation_message: msg_clone,
                result,
            };
            let _ = sender.send(task_result);
        });
        let running_task = RunningTask {
            task_handle: handle,
            started_at: std::time::Instant::now(),
            timeout,
            operation_message: message,
        };
        self.running_tasks.insert(task_id_for_running, running_task);
    }

    /// Collect completed results from running tasks and remove them from the running tasks map.
    pub fn collect_completed_results(&mut self) -> Vec<TaskResult> {
        if let Some(receiver) = &mut self.task_result_receiver {
            while let Ok(result) = receiver.try_recv() {
                self.running_tasks.remove(&result.task_id);
                self.completed_results.push(result);
            }
        }
        std::mem::take(&mut self.completed_results)
    }

    /// Check for tasks that have exceeded their timeout and handle them appropriately.
    pub fn check_timeouts(&mut self) {
        let now = std::time::Instant::now();
        let mut timed_out_tasks = Vec::new();

        for (task_id, task) in &self.running_tasks {
            if let Some(timeout_duration) = task.timeout {
                if now.duration_since(task.started_at).as_secs() > timeout_duration {
                    task.task_handle.abort();
                    timed_out_tasks.push(task_id.clone());
                }
            }
        }

        for task_id in timed_out_tasks {
            if let Some(task) = self.running_tasks.remove(&task_id) {
                let timeout_result = TaskResult {
                    task_id,
                    operation_message: task.operation_message,
                    result: Err(Error::TaskError("Operation timed out".to_string())),
                };
                self.completed_results.push(timeout_result);
            }
        }
    }

    /// Get the number of running tasks.
    pub fn running_task_count(&self) -> usize {
        self.running_tasks.len()
    }

    /// Cancel all running tasks.
    pub fn cancel_all_tasks(&mut self) {
        for (_, task) in self.running_tasks.drain() {
            task.task_handle.abort();
        }
        self.completed_results.clear();
    }
    /// List running task ids.
    pub fn list_running(&self) -> Vec<String> {
        self.running_tasks.keys().cloned().collect()
    }

    /// Note: collectors should call collect_completed_results; this provides a snapshot of queued results.
    pub fn peek_completed(&self) -> &[TaskResult] {
        &self.completed_results
    }
}
