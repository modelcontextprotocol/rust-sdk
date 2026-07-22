//! Minimal example of a server that supports the MCP Tasks extension
//! (SEP-2663, `io.modelcontextprotocol/tasks`).
//!
//! - `slow_sum` is executed as a task whenever the client declares the tasks
//!   extension capability: the server returns a `CreateTaskResult`
//!   (`resultType: "task"`) immediately and the client polls `tasks/get`.
//!   Clients that do not declare the extension get a normal synchronous
//!   response.
//! - `quick_echo` is a regular synchronous tool for contrast.
//!
//! See `examples/clients/src/task_stdio.rs` for the matching client.

#![allow(dead_code)]

use rmcp::{
    ErrorData as McpError, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::*,
    schemars,
    service::{RequestContext, RoleServer},
    task_manager::{TaskExit, TaskManager, TaskOptions},
    tool, tool_router,
};

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SumArgs {
    pub a: i32,
    pub b: i32,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct EchoArgs {
    pub message: String,
}

#[derive(Clone)]
pub struct TaskDemo {
    tool_router: ToolRouter<TaskDemo>,
    tasks: TaskManager,
}

impl Default for TaskDemo {
    fn default() -> Self {
        Self::new()
    }
}

#[tool_router]
impl TaskDemo {
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
            tasks: TaskManager::new(),
        }
    }

    /// Long-running tool. Run as a task when the client supports tasks.
    #[tool(description = "Sum two numbers after a 2-second delay")]
    async fn slow_sum(
        &self,
        Parameters(SumArgs { a, b }): Parameters<SumArgs>,
    ) -> Result<CallToolResult, McpError> {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        Ok(CallToolResult::success(vec![ContentBlock::text(
            (a + b).to_string(),
        )]))
    }

    /// Synchronous tool.
    #[tool(description = "Echo a message back immediately")]
    async fn quick_echo(
        &self,
        Parameters(EchoArgs { message }): Parameters<EchoArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(CallToolResult::success(vec![ContentBlock::text(message)]))
    }
}

impl ServerHandler for TaskDemo {
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        // SEP-2663: the server decides per-request whether to materialize a
        // task, but MUST NOT return one unless the request declared the tasks
        // extension capability.
        let client_supports_tasks = context
            .client_capabilities()
            .is_some_and(|caps| caps.supports_tasks());

        if request.name == "slow_sum" && client_supports_tasks {
            let params: SumArgs = serde_json::from_value(serde_json::Value::Object(
                request.arguments.clone().unwrap_or_default(),
            ))
            .map_err(|e| McpError::invalid_params(e.to_string(), None))?;
            let task = self.tasks.spawn(TaskOptions::default(), move |ctx| {
                Box::pin(async move {
                    // Cancellation is cooperative (SEP-2663): honor
                    // tasks/cancel by exiting with TaskExit::Cancelled.
                    tokio::select! {
                        _ = ctx.cancelled() => {
                            Err(TaskExit::Cancelled)
                        }
                        _ = tokio::time::sleep(std::time::Duration::from_secs(2)) => {
                            Ok(CallToolResult::success(vec![ContentBlock::text(
                                (params.a + params.b).to_string(),
                            )]))
                        }
                    }
                })
            });
            return Ok(CallToolResponse::Task(CreateTaskResult::new(task)));
        }

        // Fall back to synchronous execution via the tool router.
        let tcc = rmcp::handler::server::tool::ToolCallContext::new(self, request, context);
        self.tool_router.call(tcc).await
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult::with_all_items(self.tool_router.list_all()))
    }

    async fn get_task(
        &self,
        request: GetTaskParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<GetTaskResult, McpError> {
        Ok(GetTaskResult::new(self.tasks.get_task(&request.task_id)?))
    }

    async fn update_task(
        &self,
        request: UpdateTaskParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<(), McpError> {
        self.tasks
            .update_task(&request.task_id, request.input_responses)
    }

    async fn cancel_task(
        &self,
        request: CancelTaskParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<(), McpError> {
        self.tasks.cancel_task(&request.task_id)
    }

    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_tasks()
                .build(),
        )
        .with_instructions(
            "Task demo server (SEP-2663). `slow_sum` runs as a task for \
             clients that declare the tasks extension."
                .to_string(),
        )
    }
}
