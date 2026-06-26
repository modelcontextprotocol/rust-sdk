//! A receiver SHOULD NOT send a response for a request it has already been told
//! to cancel. This drives a real stdio server with raw JSON-RPC: the tool blocks
//! until the request is cancelled, so its result is only produced *after* the
//! cancellation — the service loop must drop it rather than write it to the wire.

use std::{collections::BTreeSet, process::Stdio, time::Duration};

use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler, ServiceExt,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ServerCapabilities,
        ServerInfo,
    },
    service::RequestContext,
};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader},
    process::{Child, Command},
};

const HELPER_ENV: &str = "RMCP_CANCELLED_RESPONSE_HELPER";
const READ_TIMEOUT: Duration = Duration::from_secs(10);

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelled_request_receives_no_response() -> anyhow::Result<()> {
    let mut child = spawn_helper();
    let mut writer = child.stdin.take().expect("helper stdin");
    let stdout = child.stdout.take().expect("helper stdout");
    let mut reader = BufReader::new(stdout);

    send_json(
        &mut writer,
        &json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "raw-test-client", "version": "0.0.0" }
            }
        }),
    )
    .await?;
    collect_ids_until(&mut reader, 1, READ_TIMEOUT).await?;
    send_json(
        &mut writer,
        &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
    )
    .await?;

    // Start a request that blocks until cancelled, then cancel it. Its response is
    // produced only after the cancellation arrives, so it must be suppressed.
    send_json(
        &mut writer,
        &json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": { "name": "wait-for-cancel", "arguments": {} }
        }),
    )
    .await?;
    send_json(
        &mut writer,
        &json!({
            "jsonrpc": "2.0",
            "method": "notifications/cancelled",
            "params": { "requestId": 2 }
        }),
    )
    .await?;
    // A ping proves the server is alive past the cancellation, so the absence of
    // an id=2 response is genuine suppression rather than a dead connection.
    send_json(
        &mut writer,
        &json!({ "jsonrpc": "2.0", "id": 3, "method": "ping" }),
    )
    .await?;

    let seen = collect_ids_until(&mut reader, 3, READ_TIMEOUT).await?;
    assert!(seen.contains(&3));
    assert!(!seen.contains(&2));

    drop(writer);
    wait_for_child(&mut child).await;
    Ok(())
}

struct WaitForCancelServer;

impl ServerHandler for WaitForCancelServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
    }

    async fn call_tool(
        &self,
        _request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        context.ct.cancelled().await;
        Ok(CallToolResult::success(vec![ContentBlock::text("late response")]).into())
    }
}

#[tokio::test]
async fn cancelled_response_helper() -> anyhow::Result<()> {
    if std::env::var(HELPER_ENV).as_deref() != Ok("1") {
        return Ok(());
    }
    run_helper_server().await?;
    Ok(())
}

#[cfg(feature = "local")]
async fn run_helper_server() -> anyhow::Result<()> {
    tokio::task::LocalSet::new()
        .run_until(serve_helper_stdio())
        .await
}

#[cfg(not(feature = "local"))]
async fn run_helper_server() -> anyhow::Result<()> {
    serve_helper_stdio().await
}

async fn serve_helper_stdio() -> anyhow::Result<()> {
    let server = WaitForCancelServer.serve(rmcp::transport::stdio()).await?;
    server.waiting().await?;
    Ok(())
}

fn spawn_helper() -> Child {
    let exe = std::env::current_exe().expect("current test exe");
    Command::new(exe)
        .arg("--exact")
        .arg("cancelled_response_helper")
        .arg("--quiet")
        .arg("--nocapture")
        .arg("--test-threads")
        .arg("1")
        .env(HELPER_ENV, "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn helper")
}

async fn wait_for_child(child: &mut Child) {
    let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
    if child.id().is_some() {
        let _ = child.kill().await;
    }
}

async fn send_json<W>(writer: &mut W, message: &Value) -> anyhow::Result<()>
where
    W: AsyncWrite + Unpin,
{
    let serialized = serde_json::to_string(message)?;
    writer.write_all(serialized.as_bytes()).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;
    Ok(())
}

/// Read response lines, collecting every message id seen, until `stop_id` is seen
/// (then a short grace read to catch any straggler) or the timeout elapses.
async fn collect_ids_until<R>(
    reader: &mut BufReader<R>,
    stop_id: u64,
    timeout: Duration,
) -> anyhow::Result<BTreeSet<u64>>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut seen = BTreeSet::new();
    let mut deadline = tokio::time::Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        let mut line = String::new();
        let Ok(read_result) = tokio::time::timeout(remaining, reader.read_line(&mut line)).await
        else {
            break;
        };
        if read_result? == 0 {
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };
        if let Some(id) = value.get("id").and_then(Value::as_u64) {
            seen.insert(id);
            if id == stop_id {
                // Give any late (incorrectly-sent) response a brief window to arrive.
                deadline = tokio::time::Instant::now() + Duration::from_millis(300);
            }
        }
    }
    Ok(seen)
}
