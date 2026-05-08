//! Minimal example of a tool that supports task-based invocation (SEP-1319).
//!
//! - `slow_sum` is marked `task_support = "required"`, so the client MUST invoke
//!   it as a task. The server enqueues the call into an `OperationProcessor`,
//!   returns a task id immediately, and the client polls `tasks/get` and
//!   fetches the payload via `tasks/result`.
//! - `quick_echo` is a regular synchronous tool for contrast (the default,
//!   `task_support = "forbidden"`).
//!
//! See `examples/clients/src/task_stdio.rs` for the matching client.

#![allow(dead_code)]

use std::sync::Arc;

use rmcp::{
    ErrorData as McpError, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, Content},
    schemars, task_handler,
    task_manager::OperationProcessor,
    tool, tool_handler, tool_router,
};
use tokio::sync::Mutex;

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SumArgs {
    pub a: i32,
    pub b: i32,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct EchoArgs {
    pub message: String,
}

/// Server state. The `processor` field is required by `#[task_handler]`:
/// the macro generates `enqueue_task` / `tasks/*` handlers that submit and
/// poll operations through it.
#[derive(Clone)]
pub struct TaskDemo {
    tool_router: ToolRouter<TaskDemo>,
    processor: Arc<Mutex<OperationProcessor>>,
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
            processor: Arc::new(Mutex::new(OperationProcessor::new())),
        }
    }

    /// Long-running tool. The `execution(task_support = "required")` attribute
    /// tells clients they MUST call this tool as a task; the server returns
    /// `-32601` if they don't.
    #[tool(
        description = "Sum two numbers after a 2-second delay",
        execution(task_support = "required")
    )]
    async fn slow_sum(
        &self,
        Parameters(SumArgs { a, b }): Parameters<SumArgs>,
    ) -> Result<CallToolResult, McpError> {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        Ok(CallToolResult::success(vec![Content::text(
            (a + b).to_string(),
        )]))
    }

    /// Synchronous tool with the default `task_support = "forbidden"`.
    #[tool(description = "Echo a message back immediately")]
    async fn quick_echo(
        &self,
        Parameters(EchoArgs { message }): Parameters<EchoArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(CallToolResult::success(vec![Content::text(message)]))
    }
}

/// `#[task_handler]` reads `self.processor` (configurable via the macro's
/// `processor = ...` argument) and synthesizes `enqueue_task`, `list_tasks`,
/// `get_task_info`, `get_task_result`, and `cancel_task` for us.
#[tool_handler]
#[task_handler]
impl ServerHandler for TaskDemo {}
