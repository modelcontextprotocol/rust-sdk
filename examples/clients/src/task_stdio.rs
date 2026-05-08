//! Client for the task-demo server in `examples/servers/src/task_stdio.rs`.
//!
//! Walks through the task lifecycle (SEP-1319):
//!   1. Call a regular tool (`quick_echo`) — synchronous response.
//!   2. Call a task-required tool (`slow_sum`) by attaching `task: {}` to
//!      the `tools/call` request. The server returns a `Task` with a `task_id`.
//!   3. Poll `tasks/get` until status becomes `Completed`.
//!   4. Fetch the underlying `CallToolResult` via `tasks/result`.

use anyhow::{Result, anyhow};
use rmcp::{
    ServiceExt,
    model::{
        CallToolRequestParams, CallToolResult, ClientRequest, GetTaskInfoParams,
        GetTaskResultParams, JsonObject, Request, ServerResult, TaskStatus,
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

    // Spawn the task-demo server as a child process over stdio.
    let client = ()
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

    // 1) Synchronous call. `quick_echo` has the default task_support = forbidden.
    let echo = client
        .call_tool(
            CallToolRequestParams::new("quick_echo")
                .with_arguments(object!({ "message": "hi from rmcp" })),
        )
        .await?;
    tracing::info!("quick_echo -> {echo:#?}");

    // 2) Task call. `slow_sum` is task_support = required, so we MUST attach a
    //    `task` object. An empty object is fine — clients can stash arbitrary
    //    metadata here that the server-side `OperationDescriptor` will keep.
    let create = client
        .send_request(ClientRequest::CallToolRequest(Request::new(
            CallToolRequestParams::new("slow_sum")
                .with_arguments(object!({ "a": 40, "b": 2 }))
                .with_task(JsonObject::new()),
        )))
        .await?;
    let ServerResult::CreateTaskResult(create) = create else {
        return Err(anyhow!("expected CreateTaskResult, got {create:?}"));
    };
    let task_id = create.task.task_id.clone();
    tracing::info!(
        "slow_sum enqueued as task {task_id} (status = {:?})",
        create.task.status
    );

    // 3) Poll `tasks/get` until the server reports a terminal status.
    let final_status = loop {
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;

        let info = client
            .send_request(ClientRequest::GetTaskInfoRequest(Request::new(
                GetTaskInfoParams {
                    meta: None,
                    task_id: task_id.clone(),
                },
            )))
            .await?;
        let ServerResult::GetTaskResult(info) = info else {
            return Err(anyhow!("expected GetTaskResult, got {info:?}"));
        };
        tracing::info!("status = {:?}", info.task.status);

        match info.task.status {
            TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled => {
                break info.task.status;
            }
            _ => {}
        }
    };

    if final_status != TaskStatus::Completed {
        return Err(anyhow!("task ended in {final_status:?}"));
    }

    // 4) Fetch the payload. The server-side handler returns a serialized
    //    `CallToolResult`. On the wire the response is just a JSON value, and
    //    `ServerResult` is `#[serde(untagged)]`, so the client decodes it as
    //    whichever variant the JSON shape matches first — a `CallToolResult`
    //    here. (For a non-tool task the same value would surface as
    //    `ServerResult::CustomResult` and need manual `serde_json::from_value`.)
    let payload = client
        .send_request(ClientRequest::GetTaskResultRequest(Request::new(
            GetTaskResultParams {
                meta: None,
                task_id: task_id.clone(),
            },
        )))
        .await?;
    let call_result: CallToolResult = match payload {
        ServerResult::CallToolResult(r) => r,
        ServerResult::CustomResult(c) => serde_json::from_value(c.0)?,
        other => return Err(anyhow!("unexpected task result: {other:?}")),
    };
    tracing::info!("slow_sum result -> {call_result:#?}");

    client.cancel().await?;
    Ok(())
}
