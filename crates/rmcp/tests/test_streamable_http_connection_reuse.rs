#![cfg(not(feature = "local"))]

use std::time::Instant;

use rmcp::{
    ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolRequestParams, ClientInfo, ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router,
    transport::{
        StreamableHttpClientTransport,
        streamable_http_client::StreamableHttpClientTransportConfig,
        streamable_http_server::{
            StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
        },
    },
};
use tokio_util::sync::CancellationToken;

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct SumRequest {
    a: i32,
    b: i32,
}

#[derive(Debug, Clone)]
struct EchoServer {
    tool_router: ToolRouter<Self>,
}

impl EchoServer {
    fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }
}

#[tool_router]
impl EchoServer {
    #[tool(description = "Sum two numbers")]
    fn sum(&self, Parameters(SumRequest { a, b }): Parameters<SumRequest>) -> String {
        (a + b).to_string()
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for EchoServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
    }
}

/// Verify that subsequent tool calls do not regress in latency due to
/// HTTP/1.1 connection pool exhaustion.  Before the fix, each POST SSE
/// response was dropped without fully consuming the body, preventing
/// connection reuse and forcing a new TCP connection (~40 ms) per call.
#[tokio::test]
async fn test_subsequent_tool_calls_reuse_connections() -> anyhow::Result<()> {
    let ct = CancellationToken::new();

    let service: StreamableHttpService<EchoServer, LocalSessionManager> =
        StreamableHttpService::new(
            || Ok(EchoServer::new()),
            Default::default(),
            StreamableHttpServerConfig::default()
                .with_sse_keep_alive(None)
                .with_cancellation_token(ct.child_token()),
        );

    let router = axum::Router::new().nest_service("/mcp", service);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;

    let server_handle = tokio::spawn({
        let ct = ct.clone();
        async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async move { ct.cancelled_owned().await })
                .await;
        }
    });

    let transport = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(format!("http://{addr}/mcp")),
    );
    let client = ClientInfo::default().serve(transport).await?;

    // Warm up: first call may include one-time setup costs.
    let args: serde_json::Map<String, serde_json::Value> =
        serde_json::from_value(serde_json::json!({"a": 1, "b": 2}))?;
    let _ = client
        .call_tool(CallToolRequestParams::new("sum").with_arguments(args))
        .await?;

    // Measure subsequent calls.
    let mut durations = Vec::new();
    for i in 0..5i32 {
        let args: serde_json::Map<String, serde_json::Value> =
            serde_json::from_value(serde_json::json!({"a": i, "b": i + 1}))?;
        let start = Instant::now();
        let result = client
            .call_tool(CallToolRequestParams::new("sum").with_arguments(args))
            .await?;
        let elapsed = start.elapsed();
        durations.push(elapsed);

        assert!(result.is_error != Some(true));
    }

    let _ = client.cancel().await;
    ct.cancel();
    server_handle.await?;

    // With connection reuse, localhost calls should complete well under 20 ms.
    // Before the fix, they consistently took ~42 ms due to new TCP connections.
    let max_allowed = std::time::Duration::from_millis(20);
    for d in &durations {
        assert!(*d < max_allowed);
    }

    Ok(())
}
