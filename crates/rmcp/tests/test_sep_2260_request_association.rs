#![cfg(all(feature = "server", feature = "client", not(feature = "local")))]
#![allow(deprecated)]

use std::sync::{Arc, Mutex};

use rmcp::{
    ClientHandler, RoleClient, RoleServer, ServerHandler, ServiceError, ServiceExt,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ClientInfo, ContentBlock,
        CreateMessageRequest, CreateMessageRequestParams, CreateMessageResult, ProtocolVersion,
        SamplingMessage, ServerCapabilities, ServerInfo, ServerRequest,
    },
    service::RequestContext,
};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, Lines, ReadHalf, WriteHalf},
    sync::oneshot,
};

#[derive(Clone)]
struct SamplingServer {
    outside: Arc<Mutex<Option<oneshot::Sender<Result<(), ServiceError>>>>>,
}

impl ServerHandler for SamplingServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, rmcp::ErrorData> {
        let peer = context.peer.clone();
        let slot = self.outside.clone();

        let use_generic = request.name == "sample_generic";
        tokio::spawn(async move {
            let outside = if use_generic {
                peer.send_request(ServerRequest::CreateMessageRequest(
                    CreateMessageRequest::new(CreateMessageRequestParams::new(
                        vec![SamplingMessage::user_text("standalone-generic")],
                        16,
                    )),
                ))
                .await
                .map(|_| ())
            } else {
                peer.create_message(CreateMessageRequestParams::new(
                    vec![SamplingMessage::user_text("standalone")],
                    16,
                ))
                .await
                .map(|_| ())
            };
            if let Some(tx) = slot.lock().unwrap().take() {
                let _ = tx.send(outside);
            }
        });

        let nested = context
            .peer
            .create_message(CreateMessageRequestParams::new(
                vec![SamplingMessage::user_text("nested")],
                16,
            ))
            .await;
        nested.map_err(|e| rmcp::ErrorData::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text("ok")]).into())
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

    fn get_info(&self) -> ClientInfo {
        let mut info = ClientInfo::default();
        info.protocol_version = ProtocolVersion::V_2026_07_28;
        info
    }
}

#[tokio::test]
async fn nested_sampling_allowed_standalone_rejected() -> anyhow::Result<()> {
    let (server_transport, client_transport) = tokio::io::duplex(4096);
    let (tx, rx) = oneshot::channel();
    let server = SamplingServer {
        outside: Arc::new(Mutex::new(Some(tx))),
    };
    let server_handle = tokio::spawn(async move {
        let running = server.serve(server_transport).await?;
        running.waiting().await?;
        anyhow::Ok(())
    });

    let client = SamplingClient.serve(client_transport).await?;

    let result = client
        .peer()
        .call_tool(CallToolRequestParams::new("sample"))
        .await?;
    assert_eq!(
        result.content.first().unwrap().as_text().unwrap().text,
        "ok"
    );

    let outside = rx.await?;
    assert!(matches!(outside, Err(ServiceError::McpError(_))));

    client.cancel().await?;
    let _ = server_handle.await?;
    Ok(())
}

#[tokio::test]
async fn generic_send_request_bypass_rejected() -> anyhow::Result<()> {
    let (server_transport, client_transport) = tokio::io::duplex(4096);
    let (tx, rx) = oneshot::channel();
    let server = SamplingServer {
        outside: Arc::new(Mutex::new(Some(tx))),
    };
    let server_handle = tokio::spawn(async move {
        let running = server.serve(server_transport).await?;
        running.waiting().await?;
        anyhow::Ok(())
    });

    let client = SamplingClient.serve(client_transport).await?;

    let result = client
        .peer()
        .call_tool(CallToolRequestParams::new("sample_generic"))
        .await?;
    assert_eq!(
        result.content.first().unwrap().as_text().unwrap().text,
        "ok"
    );

    let outside = rx.await?;
    assert!(
        matches!(outside, Err(ServiceError::McpError(_))),
        "generic send_request must not bypass SEP-2260 enforcement"
    );

    client.cancel().await?;
    let _ = server_handle.await?;
    Ok(())
}

// A compliant rmcp server cannot produce an unassociated server-to-client
// request at >= 2026-07-28 (send-side enforcement blocks it), so the client's
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
