#![cfg(all(feature = "server", feature = "client", not(feature = "local")))]
#![expect(deprecated, reason = "This test exercises the deprecated sampling API")]

use rmcp::{
    ClientHandler, RoleClient, RoleServer, ServerHandler, ServiceError, ServiceExt,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ClientConfig, ContentBlock,
        CreateMessageRequest, CreateMessageRequestParams, CreateMessageResult, ErrorCode,
        PingRequest, ProtocolVersion, SamplingMessage, ServerCapabilities, ServerConfig,
        ServerRequest,
    },
    service::{RequestContext, RunningService, serve_directly},
};
use serde_json::{Value, json};
use tokio::io::{
    AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, Lines, ReadHalf, WriteHalf,
};

/// Sends the server-to-client request named by the tool and reports whether it
/// was rejected as `invalid_request`.
#[derive(Clone)]
struct SamplingServer;

impl ServerHandler for SamplingServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, rmcp::ErrorData> {
        let params =
            CreateMessageRequestParams::new(vec![SamplingMessage::user_text("nested")], 16);
        let outcome = match request.name.as_ref() {
            "sample" => context.peer.create_message(params).await.map(|_| ()),
            "sample_generic" => context
                .peer
                .send_request(ServerRequest::CreateMessageRequest(
                    CreateMessageRequest::new(params),
                ))
                .await
                .map(|_| ()),
            "ping" => context
                .peer
                .send_request(ServerRequest::PingRequest(PingRequest::default()))
                .await
                .map(|_| ()),
            other => panic!("unexpected tool {other}"),
        };
        let text = match outcome {
            Err(ServiceError::McpError(e)) if e.code == ErrorCode::INVALID_REQUEST => "rejected",
            Ok(()) => "sent",
            Err(e) => return Err(rmcp::ErrorData::internal_error(e.to_string(), None)),
        };
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]).into())
    }
}

#[derive(Clone)]
struct SamplingClient;

impl ClientHandler for SamplingClient {
    async fn create_message(
        &self,
        _params: CreateMessageRequestParams,
        _context: RequestContext<RoleClient>,
    ) -> Result<CreateMessageResult, rmcp::ErrorData> {
        Ok(CreateMessageResult::new(
            SamplingMessage::assistant_text("pong"),
            "test-model".to_string(),
        )
        .with_stop_reason(CreateMessageResult::STOP_REASON_END_TURN))
    }

    fn get_info(&self) -> ClientConfig {
        let mut info = ClientConfig::default();
        info.protocol_version = ProtocolVersion::V_2026_07_28;
        info
    }
}

/// Connects the pair on `2026-07-28`. That revision dropped the `initialize`
/// handshake, so the version is agreed up front the way a discover-lifecycle
/// startup leaves it.
fn serve_modern_pair() -> (
    RunningService<RoleServer, SamplingServer>,
    RunningService<RoleClient, SamplingClient>,
) {
    let (server_transport, client_transport) = tokio::io::duplex(4096);
    let mut server_peer_info = SamplingServer.get_info();
    server_peer_info.protocol_version = ProtocolVersion::V_2026_07_28;

    let running_server = serve_directly::<RoleServer, _, _, _, _>(
        SamplingServer,
        server_transport,
        Some(SamplingClient.get_info()),
    );
    let client = serve_directly::<RoleClient, _, _, _, _>(
        SamplingClient,
        client_transport,
        Some(server_peer_info.into()),
    );
    (running_server, client)
}

async fn call_tool_on_modern_pair(tool: &'static str) -> anyhow::Result<String> {
    let (running_server, client) = serve_modern_pair();
    let server_handle = tokio::spawn(async move {
        running_server.waiting().await?;
        anyhow::Ok(())
    });

    let result = client
        .peer()
        .call_tool(CallToolRequestParams::new(tool))
        .await?;
    let text = result
        .content
        .first()
        .unwrap()
        .as_text()
        .unwrap()
        .text
        .clone();

    client.cancel().await?;
    let _ = server_handle.await?;
    Ok(text)
}

#[tokio::test]
async fn sampling_from_handler_rejected_on_modern_protocol() -> anyhow::Result<()> {
    assert_eq!(call_tool_on_modern_pair("sample").await?, "rejected");
    Ok(())
}

#[tokio::test]
async fn generic_send_request_rejected_on_modern_protocol() -> anyhow::Result<()> {
    assert_eq!(
        call_tool_on_modern_pair("sample_generic").await?,
        "rejected"
    );
    Ok(())
}

#[tokio::test]
async fn ping_rejected_on_modern_protocol() -> anyhow::Result<()> {
    assert_eq!(call_tool_on_modern_pair("ping").await?, "rejected");
    Ok(())
}

// A compliant rmcp server cannot produce a server-to-client request at
// >= 2026-07-28 (send-side enforcement blocks it), so the client's
// receive-side enforcement is exercised with a raw JSON-RPC server.
type RawServer = (
    Lines<BufReader<ReadHalf<DuplexStream>>>,
    WriteHalf<DuplexStream>,
);

async fn raw_initialize(io: DuplexStream, protocol_version: &str) -> anyhow::Result<RawServer> {
    let (read, mut write) = tokio::io::split(io);
    let mut lines = BufReader::new(read).lines();
    let init: Value = serde_json::from_str(&lines.next_line().await?.expect("initialize request"))?;
    assert_eq!(init["method"], "initialize");
    let response = json!({
        "jsonrpc": "2.0",
        "id": init["id"],
        "result": {
            "protocolVersion": protocol_version,
            "capabilities": {},
            "serverInfo": { "name": "raw-server", "version": "0.0.0" }
        }
    });
    write.write_all(format!("{response}\n").as_bytes()).await?;
    let initialized: Value =
        serde_json::from_str(&lines.next_line().await?.expect("initialized notification"))?;
    assert_eq!(initialized["method"], "notifications/initialized");
    Ok((lines, write))
}

async fn raw_request(server: &mut RawServer, request: Value) -> anyhow::Result<Value> {
    let (lines, write) = server;
    write.write_all(format!("{request}\n").as_bytes()).await?;
    Ok(serde_json::from_str(
        &lines.next_line().await?.expect("response"),
    )?)
}

fn raw_sampling_request(id: u32) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "sampling/createMessage",
        "params": {
            "messages": [{ "role": "user", "content": { "type": "text", "text": "hi" } }],
            "maxTokens": 16
        }
    })
}

#[tokio::test]
async fn unassociated_server_request_rejected_with_invalid_params() -> anyhow::Result<()> {
    let (client_io, server_io) = tokio::io::duplex(4096);
    let raw = tokio::spawn(async move {
        let mut server = raw_initialize(server_io, "2026-07-28").await?;
        raw_request(&mut server, raw_sampling_request(100)).await
    });

    let client = SamplingClient.serve(client_io).await?;
    let response = raw.await??;
    assert_eq!(
        response["error"]["code"], -32602,
        "SEP-2260: unassociated server-to-client request must be rejected with invalid params, got {response}"
    );

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn unassociated_server_request_allowed_on_legacy_protocol() -> anyhow::Result<()> {
    let (client_io, server_io) = tokio::io::duplex(4096);
    let raw = tokio::spawn(async move {
        let mut server = raw_initialize(server_io, "2025-11-25").await?;
        raw_request(&mut server, raw_sampling_request(100)).await
    });

    let client = SamplingClient.serve(client_io).await?;
    let response = raw.await??;
    assert_eq!(
        response["result"]["model"], "test-model",
        "pre-2026-07-28 peers keep the permissive behavior, got {response}"
    );

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn unassociated_ping_allowed() -> anyhow::Result<()> {
    let (client_io, server_io) = tokio::io::duplex(4096);
    let raw = tokio::spawn(async move {
        let mut server = raw_initialize(server_io, "2026-07-28").await?;
        raw_request(
            &mut server,
            json!({ "jsonrpc": "2.0", "id": 101, "method": "ping" }),
        )
        .await
    });

    let client = SamplingClient.serve(client_io).await?;
    let response = raw.await??;
    assert!(
        response.get("error").is_none(),
        "SEP-2260 excepts ping from request association, got {response}"
    );

    client.cancel().await?;
    Ok(())
}
