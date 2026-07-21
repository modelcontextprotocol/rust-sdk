//! Client for the task-demo server in `examples/servers/src/task_stdio.rs`.
//!
//! Walks through the SEP-2663 Tasks extension lifecycle:
//!   1. Call a regular tool (`quick_echo`) — synchronous response.
//!   2. Call `slow_sum` while declaring the `io.modelcontextprotocol/tasks`
//!      extension capability. The server decides to materialize a task and
//!      returns a `CreateTaskResult` (`resultType: "task"`).
//!   3. Poll `tasks/get` (honoring `pollIntervalMs`) until the task reaches a
//!      terminal status; the final `CallToolResult` is inlined in the
//!      `completed` task's `result` field.

use anyhow::{Result, anyhow};
use rmcp::{
    ServiceExt,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ClientCapabilities, GetTaskParams,
        TaskPayload, TaskStatus,
    },
    object,
    transport::{ConfigureCommandExt, TokioChildProcess},
};
use tokio::process::Command;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| format!("info,{}=debug", env!("CARGO_CRATE_NAME")).into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    // Declare the tasks extension in our client capabilities (SEP-2663).
    let client_info = rmcp::model::ClientInfo::new(
        ClientCapabilities::builder().enable_tasks().build(),
        rmcp::model::Implementation::from_build_env(),
    );

    // Spawn the task-demo server as a child process over stdio.
    let client = client_info
        .serve(TokioChildProcess::new(Command::new("cargo").configure(
            |cmd| {
                cmd.arg("run")
                    .arg("-q")
                    .arg("-p")
                    .arg("mcp-server-examples")
                    .arg("--example")
                    .arg("servers_task_stdio");
            },
        ))?)
        .await?;

    // 1) Synchronous call.
    let echo = client
        .call_tool(
            CallToolRequestParams::new("quick_echo")
                .with_arguments(object!({ "message": "hi from rmcp" })),
        )
        .await?;
    tracing::info!("quick_echo -> {echo:#?}");

    // 2) Task-eligible call. The server sees our tasks capability and
    //    materializes a task instead of blocking.
    let response = client
        .call_tool_once(
            CallToolRequestParams::new("slow_sum").with_arguments(object!({ "a": 40, "b": 2 })),
        )
        .await?;
    let create = match response {
        CallToolResponse::Task(create) => create,
        CallToolResponse::Complete(result) => {
            // The server is allowed to answer synchronously.
            tracing::info!("slow_sum answered synchronously -> {result:#?}");
            client.cancel().await?;
            return Ok(());
        }
        other => return Err(anyhow!("unexpected response: {other:?}")),
    };
    let task_id = create.task.task_id.clone();
    let poll_ms = create.task.poll_interval_ms.unwrap_or(500);
    tracing::info!(
        "slow_sum materialized as task {task_id} (status = {:?})",
        create.task.status
    );

    // 3) Poll `tasks/get` until the task reaches a terminal status.
    let final_task = loop {
        tokio::time::sleep(std::time::Duration::from_millis(poll_ms)).await;

        let info = client
            .peer()
            .get_task(GetTaskParams::new(task_id.clone()))
            .await?;
        tracing::info!("status = {:?}", info.task.status());

        if info.task.status().is_terminal() {
            break info.task;
        }
    };

    // The completed task carries the final CallToolResult inline.
    match &final_task.payload {
        TaskPayload::Completed { result } => {
            let call_result: CallToolResult =
                serde_json::from_value(serde_json::Value::Object(result.clone()))?;
            tracing::info!("slow_sum result -> {call_result:#?}");
        }
        TaskPayload::Failed { error } => {
            return Err(anyhow!("task failed: {error:?}"));
        }
        other => {
            return Err(anyhow!(
                "task ended in unexpected state {:?}",
                other.status()
            ));
        }
    }
    debug_assert_eq!(final_task.status(), TaskStatus::Completed);

    client.cancel().await?;
    Ok(())
}
