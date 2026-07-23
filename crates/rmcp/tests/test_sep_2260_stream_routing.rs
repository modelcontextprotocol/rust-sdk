//! SEP-2260 end-to-end: in-handler server→client requests ride the originating
//! POST's SSE stream, never the standalone GET stream.
#![cfg(not(feature = "local"))]

use std::{collections::BTreeMap, sync::Arc, time::Duration};

use futures::StreamExt;
use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ElicitRequestParams,
        ElicitationSchema, ServerCapabilities, ServerInfo,
    },
    service::RequestContext,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
};
use serde_json::json;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
struct ElicitingServer;

impl ServerHandler for ElicitingServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
    }

    async fn call_tool(
        &self,
        _request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        // Never answered: the test only checks the request is emitted on the right stream.
        let _ = context
            .peer
            .create_elicitation(ElicitRequestParams::FormElicitationParams {
                meta: None,
                message: "need input".to_string(),
                requested_schema: ElicitationSchema::new(BTreeMap::new()),
            })
            .await;
        Ok(CallToolResult::success(vec![ContentBlock::text("done")]).into())
    }
}

async fn start_server(ct: CancellationToken) -> String {
    let service = StreamableHttpService::new(
        move || Ok(ElicitingServer),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default().with_cancellation_token(ct.child_token()),
    );
    let router = axum::Router::new().nest_service("/mcp", service);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "http://127.0.0.1:{}/mcp",
        listener.local_addr().unwrap().port()
    );
    let ct = ct.clone();
    tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async move { ct.cancelled().await })
            .await
            .unwrap();
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    url
}

/// Read an SSE byte stream until `needle` appears or timeout.
async fn sse_contains(resp: reqwest::Response, needle: &str, timeout: Duration) -> bool {
    let mut stream = resp.bytes_stream();
    tokio::time::timeout(timeout, async {
        let mut buffer = String::new();
        while let Some(Ok(chunk)) = stream.next().await {
            buffer.push_str(&String::from_utf8_lossy(&chunk));
            if buffer.contains(needle) {
                return true;
            }
        }
        false
    })
    .await
    .unwrap_or(false)
}

#[tokio::test]
async fn elicitation_rides_originating_post_stream_not_standalone_get() {
    let ct = CancellationToken::new();
    let url = start_server(ct.clone()).await;
    let client = reqwest::Client::new();

    // 2025-11-25 is the latest session-carrying version; SEP-2567 serves
    // 2026-07-28+ statelessly, with no standalone GET stream to test against.
    let resp = client
        .post(&url)
        .header("Accept", "text/event-stream, application/json")
        .header("Content-Type", "application/json")
        .json(&json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": { "elicitation": {} },
                "clientInfo": { "name": "test-client", "version": "1.0.0" }
            }
        }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());
    let session_id = resp
        .headers()
        .get("Mcp-Session-Id")
        .expect("session id")
        .to_str()
        .unwrap()
        .to_string();

    client
        .post(&url)
        .header("Accept", "text/event-stream, application/json")
        .header("Content-Type", "application/json")
        .header("Mcp-Session-Id", &session_id)
        .json(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
        .send()
        .await
        .unwrap();

    // Standalone GET stream — must NEVER carry the elicitation request.
    let get_stream = client
        .get(&url)
        .header("Accept", "text/event-stream")
        .header("Mcp-Session-Id", &session_id)
        .send()
        .await
        .unwrap();
    assert_eq!(get_stream.status(), 200);

    // The in-handler elicitation must appear on this tools/call SSE stream.
    let post_stream = client
        .post(&url)
        .header("Accept", "text/event-stream, application/json")
        .header("Content-Type", "application/json")
        .header("Mcp-Session-Id", &session_id)
        .json(&json!({
            "jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": { "name": "ask", "arguments": {} }
        }))
        .send()
        .await
        .unwrap();
    assert!(post_stream.status().is_success());

    assert!(
        sse_contains(post_stream, "elicitation/create", Duration::from_secs(5)).await,
        "elicitation request must be delivered on the originating POST SSE stream"
    );
    assert!(
        !sse_contains(get_stream, "elicitation/create", Duration::from_millis(500)).await,
        "standalone GET stream must not carry the elicitation request"
    );

    ct.cancel();
}
