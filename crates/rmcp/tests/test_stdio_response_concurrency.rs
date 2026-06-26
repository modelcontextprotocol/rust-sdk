#![cfg(not(feature = "local"))]

use std::{collections::BTreeSet, process::Stdio, time::Duration};

use rmcp::{
    ErrorData as McpError, ServerHandler, ServiceExt,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ServerCapabilities,
        ServerInfo,
    },
};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader},
    process::{Child, Command},
};

const HELPER_ENV: &str = "RMCP_STDIO_RESPONSE_CONCURRENCY_HELPER";
const REQUESTS: usize = 200;
const RESPONSE_BYTES: usize = 64 * 1024;
const READ_TIMEOUT: Duration = Duration::from_secs(10);

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn raw_client_concurrent_large_stdio_tool_responses_are_not_lost() -> anyhow::Result<()> {
    // Spawn the same test binary as a child process so the server uses real
    // stdio pipes, not an in-process transport.
    let mut child = spawn_helper();
    let mut writer = child.stdin.take().expect("helper stdin");
    let stdout = child.stdout.take().expect("helper stdout");
    let mut reader = BufReader::new(stdout);

    // Complete the normal MCP initialization flow before stressing tools/call.
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
    read_response_for_id(&mut reader, 1).await?;

    send_json(
        &mut writer,
        &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
    )
    .await?;

    // Send the whole batch before reading responses. This creates concurrent
    // request handling and concurrent response production inside rmcp.
    for id in request_ids() {
        send_json(
            &mut writer,
            &json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "tools/call",
                "params": { "name": "large-response", "arguments": {} }
            }),
        )
        .await?;
    }

    let missing_ids = read_responses_for_ids(&mut reader, request_ids(), READ_TIMEOUT).await?;
    assert!(
        missing_ids.is_empty(),
        "missing response ids: {missing_ids:?}"
    );

    drop(writer);
    wait_for_child(&mut child).await;
    Ok(())
}

struct LargeResponseServer;

impl ServerHandler for LargeResponseServer {
    #[allow(deprecated)]
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        assert_eq!("large-response", request.name.as_ref());
        Ok(CallToolResult::success(vec![ContentBlock::text("x".repeat(RESPONSE_BYTES))]).into())
    }
}

#[tokio::test]
async fn stdio_response_concurrency_helper() -> anyhow::Result<()> {
    // The parent test starts this same binary with HELPER_ENV=1 so it can act
    // as a small MCP server connected over real stdin/stdout pipes.
    if std::env::var(HELPER_ENV).as_deref() != Ok("1") {
        return Ok(());
    }
    let server = LargeResponseServer.serve(rmcp::transport::stdio()).await?;
    server.waiting().await?;
    Ok(())
}

fn spawn_helper() -> Child {
    let exe = std::env::current_exe().expect("current test exe");
    Command::new(exe)
        .arg("--exact")
        .arg("stdio_response_concurrency_helper")
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

fn request_ids() -> BTreeSet<u64> {
    (1000..1000 + REQUESTS as u64).collect()
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

async fn read_response_for_id<R>(reader: &mut BufReader<R>, expected_id: u64) -> anyhow::Result<()>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let missing =
        read_responses_for_ids(reader, BTreeSet::from([expected_id]), READ_TIMEOUT).await?;
    if missing.is_empty() {
        Ok(())
    } else {
        anyhow::bail!("missing response id {expected_id}")
    }
}

async fn read_responses_for_ids<R>(
    reader: &mut BufReader<R>,
    mut pending_ids: BTreeSet<u64>,
    timeout: Duration,
) -> anyhow::Result<BTreeSet<u64>>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let deadline = tokio::time::Instant::now() + timeout;
    while !pending_ids.is_empty() {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        let mut line = String::new();
        let Ok(read_result) = tokio::time::timeout(remaining, reader.read_line(&mut line)).await
        else {
            break;
        };
        let read = read_result?;
        if read == 0 {
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
            pending_ids.remove(&id);
        }
    }
    Ok(pending_ids)
}
