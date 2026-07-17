#![cfg(all(
    feature = "server",
    feature = "transport-streamable-http-server",
    feature = "reqwest",
    not(feature = "local")
))]

//! Regression test for #857: when a stateless streamable-HTTP client disconnects
//! (drops the response) while a tool handler is still awaiting, the per-request
//! `RequestContext::ct` should fire so the handler can cancel cooperatively.
//!
//! Stateless requests are one-shot (no session, no resumption), so a dropped
//! response is terminal and safe to cancel — unlike the stateful/resumable path,
//! where a disconnect may be recovered via `Last-Event-ID`.

use std::{sync::Arc, time::Duration};

use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ServerCapabilities,
        ServerInfo,
    },
    service::RequestContext,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
struct CancelProbe {
    started: Arc<Notify>,
    cancelled: Arc<Notify>,
}

impl ServerHandler for CancelProbe {
    #[allow(deprecated)]
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
    }

    async fn call_tool(
        &self,
        _request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        self.started.notify_one();
        // Wait until the per-request cancellation token fires, or give up after a
        // generous timeout so a buggy build fails via the outer assertion rather
        // than hanging the test.
        tokio::select! {
            _ = context.ct.cancelled() => {
                self.cancelled.notify_one();
                Ok(CallToolResult::success(vec![ContentBlock::text("cancelled")]).into())
            }
            _ = tokio::time::sleep(Duration::from_secs(30)) => {
                Ok(CallToolResult::success(vec![ContentBlock::text("ran_to_completion")]).into())
            }
        }
    }
}

const CALL_BODY: &str = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"wait_for_cancel","arguments":{}}}"#;

struct TestServer {
    url: String,
    server_ct: CancellationToken,
    started: Arc<Notify>,
    cancelled: Arc<Notify>,
}

async fn spawn_stateless_server(json_response: bool) -> anyhow::Result<TestServer> {
    let started = Arc::new(Notify::new());
    let cancelled = Arc::new(Notify::new());
    let probe = CancelProbe {
        started: started.clone(),
        cancelled: cancelled.clone(),
    };

    let server_ct = CancellationToken::new();
    let config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(json_response)
        // A short keep-alive lets the SSE server notice a dropped connection
        // quickly (hyper only observes the disconnect on its next write).
        .with_sse_keep_alive(Some(Duration::from_millis(100)))
        .with_cancellation_token(server_ct.child_token());

    let service: StreamableHttpService<CancelProbe, LocalSessionManager> =
        StreamableHttpService::new(
            move || Ok(probe.clone()),
            Arc::new(LocalSessionManager::default()),
            config,
        );
    let router = axum::Router::new().nest_service("/mcp", service);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    tokio::spawn({
        let ct = server_ct.clone();
        async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async move { ct.cancelled_owned().await })
                .await;
        }
    });

    Ok(TestServer {
        url: format!("http://{addr}/mcp"),
        server_ct,
        started,
        cancelled,
    })
}

/// SSE mode: the response is a stream; dropping it (client disconnect) must fire
/// the handler's cancellation token.
#[tokio::test]
async fn stateless_sse_client_disconnect_cancels_request() -> anyhow::Result<()> {
    let server = spawn_stateless_server(false).await?;
    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(0)
        .build()?;

    // A single self-contained tools/call (no session, no initialize handshake).
    let call = client
        .post(&server.url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2025-03-26")
        .body(CALL_BODY)
        .send()
        .await?;
    assert!(
        call.status().is_success(),
        "tools/call failed: {:?}",
        call.status()
    );

    tokio::time::timeout(Duration::from_secs(5), server.started.notified())
        .await
        .expect("tool handler should start");

    // Client disconnects mid-call: drop the streaming response (and the client).
    drop(call);
    drop(client);

    tokio::time::timeout(Duration::from_secs(10), server.cancelled.notified())
        .await
        .expect("RequestContext::ct should fire after client disconnect (SSE)");

    server.server_ct.cancel();
    Ok(())
}

/// JSON-direct mode: the server holds the connection open awaiting the single
/// response. A client that disconnects while the handler is running must still
/// fire the handler's cancellation token.
#[tokio::test]
async fn stateless_json_client_disconnect_cancels_request() -> anyhow::Result<()> {
    let server = spawn_stateless_server(true).await?;
    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(0)
        .build()?;

    // In JSON mode the server does not respond until the handler completes, so
    // the request stays pending; drive it from a task we can abort to disconnect.
    let url = server.url.clone();
    let req_task = tokio::spawn(async move {
        let _ = client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .header("MCP-Protocol-Version", "2025-03-26")
            .body(CALL_BODY)
            .send()
            .await;
        // Keep the client alive until the request future is dropped by abort().
        drop(client);
    });

    tokio::time::timeout(Duration::from_secs(5), server.started.notified())
        .await
        .expect("tool handler should start");

    // Client disconnects: abort the in-flight request, closing the connection.
    req_task.abort();

    tokio::time::timeout(Duration::from_secs(10), server.cancelled.notified())
        .await
        .expect("RequestContext::ct should fire after client disconnect (JSON)");

    server.server_ct.cancel();
    Ok(())
}
