#![cfg(all(
    not(feature = "local"),
    feature = "client",
    feature = "reqwest",
    feature = "transport-streamable-http-server"
))]
#![expect(deprecated, reason = "This test exercises the deprecated sampling API")]

use std::{borrow::Cow, sync::Arc, time::Duration};

use rmcp::{
    ClientHandler, ClientLifecycleMode, ClientServiceExt, ServerHandler,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock,
        CreateMessageRequestParams, CreateMessageResult, ErrorCode, ProtocolVersion,
        SamplingMessage, ServerCapabilities, ServerConfig,
    },
    service::{RequestContext, RoleClient, RoleServer},
    transport::{
        StreamableHttpClientTransport,
        streamable_http_client::StreamableHttpClientTransportConfig,
        streamable_http_server::{
            StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
        },
    },
};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
struct SamplingServer;

impl ServerHandler for SamplingServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
    }

    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Borrowed(&[ProtocolVersion::V_2026_07_28])
    }

    async fn call_tool(
        &self,
        _request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, rmcp::ErrorData> {
        context
            .peer
            .create_message(CreateMessageRequestParams::new(
                vec![SamplingMessage::user_text("Should we proceed?")],
                64,
            ))
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text("done")]).into())
    }
}

#[derive(Clone)]
struct AnsweringClient;

impl ClientHandler for AnsweringClient {
    async fn create_message(
        &self,
        _params: CreateMessageRequestParams,
        _context: RequestContext<RoleClient>,
    ) -> Result<CreateMessageResult, rmcp::ErrorData> {
        Ok(CreateMessageResult::new(
            SamplingMessage::assistant_text("yes"),
            "test-model".to_string(),
        ))
    }
}

async fn spawn_server(ct: &CancellationToken) -> String {
    let service = StreamableHttpService::new(
        || Ok(SamplingServer),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default()
            .with_sse_keep_alive(None)
            .with_cancellation_token(ct.child_token()),
    );
    let router = axum::Router::new().nest_service("/mcp", service);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener should bind");
    let address = listener.local_addr().expect("listener address");
    let ct = ct.clone();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router)
            .with_graceful_shutdown(async move { ct.cancelled_owned().await })
            .await;
    });
    format!("http://{address}/mcp")
}

#[tokio::test]
async fn sampling_from_stateless_handler_fails_instead_of_hanging() {
    let ct = CancellationToken::new();
    let url = spawn_server(&ct).await;
    let client = AnsweringClient
        .serve_with_lifecycle(
            StreamableHttpClientTransport::from_config(
                StreamableHttpClientTransportConfig::with_uri(url),
            ),
            ClientLifecycleMode::Discover {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            },
        )
        .await
        .expect("discover should succeed");

    let outcome = tokio::time::timeout(
        Duration::from_secs(5),
        client.call_tool(CallToolRequestParams::new("ask")),
    )
    .await
    .expect("tool call must not hang");

    let error = outcome.expect_err("sampling must be rejected on 2026-07-28");
    assert!(
        error.to_string().contains("InputRequiredResult"),
        "unexpected error: {error}"
    );
    client.cancel().await.expect("cancel client");
    ct.cancel();
}

#[tokio::test]
async fn stateless_server_rejects_posted_response() {
    let ct = CancellationToken::new();
    let url = spawn_server(&ct).await;

    let response = reqwest::Client::new()
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2026-07-28")
        .json(&json!({ "jsonrpc": "2.0", "id": 0, "result": {} }))
        .send()
        .await
        .expect("send response POST");

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    let body: Value = response.json().await.expect("JSON-RPC error body");
    assert_eq!(
        body["error"]["code"],
        json!(ErrorCode::INVALID_REQUEST.0),
        "unexpected body: {body}"
    );
    ct.cancel();
}
