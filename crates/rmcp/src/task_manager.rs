use std::{collections::HashMap, pin::Pin};

use futures::Future;
use tokio::sync::mpsc;

use crate::{error::RmcpError as Error, model::ClientRequest, service::RequestContext, RoleServer};

/// Boxed future that represents an asynchronous operation managed by the processor.
pub type OperationFuture = Pin<
    Box<dyn Future<Output = Result<Box<dyn OperationResultTransport>, Error>> + Send>,
>;

/// Describes metadata associated with an enqueued task.
#[derive(Debug, Clone)]
pub struct OperationDescriptor {
    pub operation_id: String,
    pub name: String,
    pub client_request: Option<ClientRequest>,
    pub context: Option<RequestContext<RoleServer>>,
    pub timeout_secs: Option<u64>,
}

impl OperationDescriptor {
    pub fn new(operation_id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            operation_id: operation_id.into(),
            name: name.into(),
            client_request: None,
            context: None,
            timeout_secs: None,
        }
    }

    pub fn with_client_request(mut self, request: ClientRequest) -> Self {
        self.client_request = Some(request);
        self
    }

    pub fn with_context(mut self, context: RequestContext<RoleServer>) -> Self {
        self.context = Some(context);
        self
    }

    pub fn with_timeout(mut self, timeout_secs: u64) -> Self {
        self.timeout_secs = Some(timeout_secs);
        self
    }
}

/// Operation message describing a unit of asynchronous work.
pub struct OperationMessage {
    pub descriptor: OperationDescriptor,
    pub future: OperationFuture,
}

impl OperationMessage {
    pub fn new(descriptor: OperationDescriptor, future: OperationFuture) -> Self {
        Self { descriptor, future }
    }
}

/// Trait for operation result transport
pub trait OperationResultTransport: Send + Sync + 'static {
    fn operation_id(&self) -> &String;
    fn as_any(&self) -> &dyn std::any::Any;
}

// ===== Operation Processor =====
pub const DEFAULT_TASK_TIMEOUT_SECS: u64 = 300; // 5 minutes
/// Operation processor that coordinates extractors and handlers
pub struct OperationProcessor {
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
    descriptor: OperationDescriptor,
}

pub struct TaskResult {
    pub descriptor: OperationDescriptor,
    pub result: Result<Box<dyn OperationResultTransport>, Error>,
}

impl OperationProcessor {
    pub fn new() -> Self {
        let (task_result_sender, task_result_receiver) = mpsc::unbounded_channel();
        Self {
            running_tasks: HashMap::new(),
            completed_results: Vec::new(),
            task_result_receiver: Some(task_result_receiver),
            task_result_sender,
        }
    }

    /// Submit an operation for asynchronous execution.
    pub fn submit_operation(&mut self, message: OperationMessage) -> Result<(), Error> {
        if self
            .running_tasks
            .contains_key(&message.descriptor.operation_id)
        {
            return Err(Error::TaskError(format!(
                "Operation with id {} is already running",
                message.descriptor.operation_id
            )));
        }
        self.spawn_async_task(message);
        Ok(())
    }

    fn spawn_async_task(&mut self, message: OperationMessage) {
        let OperationMessage { descriptor, future } = message;
        let task_id = descriptor.operation_id.clone();
        let timeout = descriptor
            .timeout_secs
            .or(Some(DEFAULT_TASK_TIMEOUT_SECS));
        let sender = self.task_result_sender.clone();
        let descriptor_for_result = descriptor.clone();
        let handle = tokio::spawn(async move {
            let result = future.await;
            let task_result = TaskResult {
                descriptor: descriptor_for_result,
                result,
            };
            let _ = sender.send(task_result);
        });
        let running_task = RunningTask {
            task_handle: handle,
            started_at: std::time::Instant::now(),
            timeout,
            descriptor,
        };
        self.running_tasks.insert(task_id, running_task);
    }

    /// Collect completed results from running tasks and remove them from the running tasks map.
    pub fn collect_completed_results(&mut self) -> Vec<TaskResult> {
        if let Some(receiver) = &mut self.task_result_receiver {
            while let Ok(result) = receiver.try_recv() {
                self
                    .running_tasks
                    .remove(&result.descriptor.operation_id);
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
                    descriptor: task.descriptor,
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
