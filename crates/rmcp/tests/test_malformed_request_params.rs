//! A request for a spec method whose params do not match the schema falls back
//! to `ClientRequest::CustomRequest` during deserialization. The server must
//! answer it with -32602 Invalid params: the method exists, so -32601 Method not
//! found is wrong (and on modern Streamable HTTP it also becomes a 404).
#![cfg(all(feature = "server", not(feature = "local")))]

use std::time::Duration;

use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler, ServiceExt,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ServerCapabilities,
        ServerConfig,
    },
    service::RequestContext,
};
use serde_json::{Value, json};
use tokio::io::{
    AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader, DuplexStream, Lines, ReadHalf,
};

#[derive(Debug, Clone, Default)]
struct ToolServer;

impl ServerHandler for ToolServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
    }

    async fn call_tool(
        &self,
        _request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        Ok(CallToolResult::success(vec![ContentBlock::text("ok")]).into())
    }
}

async fn send(write: &mut (impl AsyncWrite + Unpin), message: Value) -> anyhow::Result<()> {
    let mut line = serde_json::to_vec(&message)?;
    line.push(b'\n');
    write.write_all(&line).await?;
    Ok(())
}

async fn recv(lines: &mut Lines<BufReader<ReadHalf<DuplexStream>>>) -> anyhow::Result<Value> {
    let line = tokio::time::timeout(Duration::from_secs(5), lines.next_line())
        .await??
        .ok_or_else(|| anyhow::anyhow!("server closed the stream"))?;
    Ok(serde_json::from_str(&line)?)
}

#[tokio::test]
async fn malformed_params_for_spec_method_is_invalid_params() -> anyhow::Result<()> {
    let (server_io, client_io) = tokio::io::duplex(4096);
    let server = tokio::spawn(async move {
        let running = ToolServer.serve(server_io).await?;
        running.waiting().await?;
        anyhow::Ok(())
    });
    let (read, mut write) = tokio::io::split(client_io);
    let mut lines = BufReader::new(read).lines();

    send(
        &mut write,
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "raw-client", "version": "0.0.0" }
            }
        }),
    )
    .await?;
    assert_eq!(recv(&mut lines).await?["id"], 1);
    send(
        &mut write,
        json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
    )
    .await?;

    // `name` is required by CallToolRequest; `arguments` must be an object.
    for (id, params) in [
        (2, json!({ "arguments": {} })),
        (3, json!({ "name": "echo", "arguments": "not-an-object" })),
    ] {
        send(
            &mut write,
            json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call", "params": params }),
        )
        .await?;
        let response = recv(&mut lines).await?;
        assert_eq!(response["id"], id, "{response}");
        assert_eq!(response["error"]["code"], -32602, "{response}");
    }

    // The tool itself works, and an unknown method is still -32601.
    send(
        &mut write,
        json!({
            "jsonrpc": "2.0", "id": 4, "method": "tools/call",
            "params": { "name": "echo", "arguments": {} }
        }),
    )
    .await?;
    let response = recv(&mut lines).await?;
    assert_eq!(response["id"], 4, "{response}");
    assert!(response.get("result").is_some(), "{response}");

    send(
        &mut write,
        json!({ "jsonrpc": "2.0", "id": 5, "method": "no/such/method" }),
    )
    .await?;
    let response = recv(&mut lines).await?;
    assert_eq!(response["id"], 5, "{response}");
    assert_eq!(response["error"]["code"], -32601, "{response}");

    drop(write);
    drop(lines);
    server.await??;
    Ok(())
}
