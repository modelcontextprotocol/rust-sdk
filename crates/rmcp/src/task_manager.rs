use std::{any::Any, collections::HashMap};

use futures::{
    Future, FutureExt, StreamExt,
    future::abortable,
    stream::{AbortHandle, FuturesUnordered},
};
use tokio::{
    sync::mpsc,
    time::{Duration, timeout},
};

use crate::{
    RoleServer,
    error::{ErrorData as McpError, RmcpError as Error},
    model::{CallToolResult, ClientRequest},
    service::RequestContext,
    util::PinnedFuture,
};

/// Result of running an operation
pub type OperationResult = Result<Box<dyn OperationResultTransport>, Error>;

/// Boxed future that represents an asynchronous operation managed by the processor.
pub type OperationFuture<'a> = PinnedFuture<'a, OperationResult>;

/// Describes metadata associated with an enqueued task.
#[derive(Debug, Clone)]
pub struct OperationDescriptor {
    pub operation_id: String,
    pub name: String,
    pub client_request: Option<ClientRequest>,
    pub context: Option<RequestContext<RoleServer>>,
    pub ttl: Option<u64>,
}

impl OperationDescriptor {
    pub fn new(operation_id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            operation_id: operation_id.into(),
            name: name.into(),
            client_request: None,
            context: None,
            ttl: None,
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

    pub fn with_ttl(mut self, ttl: u64) -> Self {
        self.ttl = Some(ttl);
        self
    }
}

/// Operation message describing a unit of asynchronous work.
pub struct OperationMessage {
    pub descriptor: OperationDescriptor,
    pub future: OperationFuture<'static>,
}

impl OperationMessage {
    pub fn new(descriptor: OperationDescriptor, future: OperationFuture<'static>) -> Self {
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
    /// Receiver for asynchronously completed task results. Used
    /// to collect back into `completed_results`
    task_result_receiver: mpsc::UnboundedReceiver<TaskResult>,
    /// Sender to spawn futures on the worker task associated with this
    /// processor. The worker future is created as part of [OperationProcessor::new]
    spawn_tx: mpsc::UnboundedSender<(OperationDescriptor, OperationFuture<'static>)>,
}

/// A handle to a running operation.
struct RunningTask {
    task_handle: AbortHandle,
    started_at: std::time::Instant,
    timeout: Option<u64>,
    descriptor: OperationDescriptor,
}

/// The result of a running operation.
pub struct TaskResult {
    pub descriptor: OperationDescriptor,
    pub result: Result<Box<dyn OperationResultTransport>, Error>,
}

/// Helper to generate an ISO 8601 timestamp for task metadata.
pub fn current_timestamp() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Result transport for tool calls executed as tasks.
pub struct ToolCallTaskResult {
    id: String,
    pub result: Result<CallToolResult, McpError>,
}

impl ToolCallTaskResult {
    pub fn new(id: impl Into<String>, result: Result<CallToolResult, McpError>) -> Self {
        Self {
            id: id.into(),
            result,
        }
    }
}

impl OperationResultTransport for ToolCallTaskResult {
    fn operation_id(&self) -> &String {
        &self.id
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl OperationProcessor {
    /// Create a new operation processor.
    ///
    /// This function will return the new [OperationProcessor]
    /// facade you can use to queue operations, and also a future
    /// that must be polled to handle these operations.
    ///
    /// Spawn the work function on your runtime of choice, or poll it
    /// manually.
    pub fn new() -> (Self, impl Future<Output = ()>) {
        let (task_result_sender, task_result_receiver) = mpsc::unbounded_channel();
        let (spawn_tx, mut spawn_rx) =
            mpsc::unbounded_channel::<(OperationDescriptor, OperationFuture)>();

        let work = async move {
            let mut work_set =
                FuturesUnordered::<PinnedFuture<(OperationDescriptor, OperationResult)>>::new();

            // Loop and listen for new operations incoming that need to be added to the future pool,
            // and also listen to operation completions via the future pool.
            loop {
                tokio::select! {
                    spawn_req = spawn_rx.recv(), if !spawn_rx.is_closed() => {
                        if let Some((descriptor, fut)) = spawn_req {
                            // Map the future back to a descriptor and result tuple
                            let operation_work = fut.map(|result| (descriptor, result)).boxed();
                            // Add it to the set we are polling
                            work_set.push(operation_work);
                        }
                    },
                    operation_result = work_set.next(), if !work_set.is_empty() => {
                        if let Some((descriptor, result)) = operation_result {
                            match task_result_sender.send(TaskResult { descriptor, result }) {
                                Err(e) => {
                                    // TODO: Produce an error message here!
                                }
                                _ => {}
                            }
                        };
                    },
                    else => {
                        // Work was empty, and spawn channel was closed. Time
                        // to break the loop.
                        break;
                    }
                }
            }
        };

        let this = Self {
            running_tasks: HashMap::new(),
            completed_results: Vec::new(),
            task_result_receiver,
            spawn_tx,
        };

        (this, work)
    }

    /// Submit an operation for asynchronous execution.
    #[allow(clippy::result_large_err)]
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

    /// Spawns an operation to be executed to completion.
    fn spawn_async_task(&mut self, message: OperationMessage) {
        let OperationMessage { descriptor, future } = message;
        let task_id = descriptor.operation_id.clone();
        let timeout_secs = descriptor.ttl.or(Some(DEFAULT_TASK_TIMEOUT_SECS));

        let timed_future = async move {
            if let Some(secs) = timeout_secs {
                match timeout(Duration::from_secs(secs), future).await {
                    Ok(result) => result,
                    Err(_) => Err(Error::TaskError("Operation timed out".to_string())),
                }
            } else {
                future.await
            }
        };

        // Below, we want to give the user a handle to the long-running operation,
        // but we don't want to send the result to the user's handle. Rather the
        // result gets consumed in the worker task created in the `Self::new`
        // function. So here we will use the `Abortable` future utility.
        let (work, abort_handle) = abortable(timed_future);

        // Map the error type of abortion (for now)
        let work = work.map(|result| {
            match result {
                // Was not aborted, true operation result
                Ok(inner_result) => inner_result,
                // Was aborted, flatten to expected error type
                Err(e) => Err(Error::TaskError(e.to_string())),
            }
        });

        // Then send the work to be executed
        match self.spawn_tx.send((descriptor.clone(), work.boxed())) {
            Ok(_) => {}
            Err(e) => {
                // TODO: Produce an error message!
            }
        }

        let running_task = RunningTask {
            task_handle: abort_handle,
            started_at: std::time::Instant::now(),
            timeout: timeout_secs,
            descriptor,
        };
        self.running_tasks.insert(task_id, running_task);
    }

    /// Collect completed results from running tasks and remove them from the running tasks map.
    fn collect_completed_results(&mut self) {
        while let Ok(result) = self.task_result_receiver.try_recv() {
            self.running_tasks.remove(&result.descriptor.operation_id);
            self.completed_results.push(result);
        }
    }

    /// Check for tasks that have exceeded their timeout and handle them appropriately.
    pub fn check_timeouts(&mut self) {
        self.collect_completed_results();
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
    pub fn running_task_count(&mut self) -> usize {
        self.collect_completed_results();
        self.running_tasks.len()
    }

    /// Cancel all running tasks.
    pub fn cancel_all_tasks(&mut self) {
        for (_, task) in self.running_tasks.drain() {
            task.task_handle.abort();
        }
        while self.task_result_receiver.try_recv().is_ok() {}
        self.completed_results.clear();
    }

    /// List running task ids.
    pub fn list_running(&mut self) -> Vec<String> {
        self.collect_completed_results();
        self.running_tasks.keys().cloned().collect()
    }

    /// Returns a snapshot of completed task results.
    pub fn peek_completed(&mut self) -> &[TaskResult] {
        self.collect_completed_results();
        &self.completed_results
    }

    /// Fetch the metadata for a running or recently completed task.
    pub fn task_descriptor(&self, task_id: &str) -> Option<&OperationDescriptor> {
        if let Some(task) = self.running_tasks.get(task_id) {
            return Some(&task.descriptor);
        }
        self.completed_results
            .iter()
            .rev()
            .find(|result| result.descriptor.operation_id == task_id)
            .map(|result| &result.descriptor)
    }

    /// Attempt to cancel a running task.
    pub fn cancel_task(&mut self, task_id: &str) -> bool {
        self.collect_completed_results();
        if let Some(task) = self.running_tasks.remove(task_id) {
            task.task_handle.abort();
            // Insert a cancelled result so callers can observe the terminal state.
            let cancel_result = TaskResult {
                descriptor: task.descriptor,
                result: Err(Error::TaskError("Operation cancelled".to_string())),
            };
            self.completed_results.push(cancel_result);
            return true;
        }
        false
    }

    /// Retrieve a completed task result if available.
    pub fn take_completed_result(&mut self, task_id: &str) -> Option<TaskResult> {
        self.collect_completed_results();
        if let Some(position) = self
            .completed_results
            .iter()
            .position(|result| result.descriptor.operation_id == task_id)
        {
            Some(self.completed_results.remove(position))
        } else {
            None
        }
    }
}
