#![cfg(not(feature = "local"))]
use std::time::Duration;

use futures::future::BoxFuture;
use rmcp::{
    ServerHandler, ServiceExt,
    handler::server::{
        router::tool::ToolRoute,
        tool::{ToolCallContext, ToolRouter, schema_for_type},
    },
    model::{
        CallToolRequestParams, CallToolResult, Content, ProgressNotificationParam,
        ServerCapabilities, ServerInfo, Tool,
    },
    tool_handler,
    transport::{
        StreamableHttpClientTransport,
        streamable_http_client::StreamableHttpClientTransportConfig,
        streamable_http_server::{
            StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
        },
    },
};
use tokio_util::sync::CancellationToken;

mod common;
use common::calculator::Calculator;

const INIT_BODY: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}"#;

async fn spawn_server(
    config: StreamableHttpServerConfig,
) -> (reqwest::Client, String, CancellationToken) {
    let ct = config.cancellation_token.clone();
    let service: StreamableHttpService<Calculator, LocalSessionManager> =
        StreamableHttpService::new(|| Ok(Calculator::new()), Default::default(), config);

    let router = axum::Router::new().nest_service("/mcp", service);
    let tcp_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = tcp_listener.local_addr().unwrap();

    tokio::spawn({
        let ct = ct.clone();
        async move {
            let _ = axum::serve(tcp_listener, router)
                .with_graceful_shutdown(async move { ct.cancelled_owned().await })
                .await;
        }
    });

    let client = reqwest::Client::new();
    let base_url = format!("http://{addr}/mcp");
    (client, base_url, ct)
}

#[tokio::test]
async fn stateless_json_response_returns_application_json() -> anyhow::Result<()> {
    let ct = CancellationToken::new();
    let (client, url, ct) = spawn_server(StreamableHttpServerConfig {
        stateful_mode: false,
        json_response: true,
        sse_keep_alive: None,
        cancellation_token: ct.child_token(),
        ..Default::default()
    })
    .await;

    let response = client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .body(INIT_BODY)
        .send()
        .await?;

    assert_eq!(response.status(), 200);

    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        content_type.contains("application/json"),
        "Expected application/json, got: {content_type}"
    );

    let body = response.text().await?;
    let parsed: serde_json::Value = serde_json::from_str(&body)?;
    assert_eq!(parsed["jsonrpc"], "2.0");
    assert_eq!(parsed["id"], 1);
    assert!(parsed["result"].is_object(), "Expected result object");

    ct.cancel();
    Ok(())
}

#[derive(Debug, Default, serde::Deserialize, schemars::JsonSchema)]
struct EmptyArgs {}

#[derive(Debug, Clone)]
struct ProgressToolServer {
    tool_router: ToolRouter<Self>,
}

impl ProgressToolServer {
    fn new() -> Self {
        Self {
            tool_router: ToolRouter::new().with_route(ToolRoute::new_dyn(
                Tool::new(
                    "progress_then_result",
                    "Emit a progress notification before returning",
                    schema_for_type::<EmptyArgs>(),
                ),
                |context: ToolCallContext<'_, Self>| -> BoxFuture<'_, _> {
                    Box::pin(async move {
                        let Some(progress_token) =
                            context.request_context.meta.get_progress_token()
                        else {
                            return Err(rmcp::ErrorData::invalid_params(
                                "missing progress token",
                                None,
                            ));
                        };

                        context
                            .request_context
                            .peer
                            .notify_progress(ProgressNotificationParam::new(progress_token, 1.0))
                            .await
                            .map_err(|err| {
                                rmcp::ErrorData::internal_error(
                                    format!("failed to send progress notification: {err}"),
                                    None,
                                )
                            })?;

                        Ok(CallToolResult::success(vec![Content::text("done")]))
                    })
                },
            )),
        }
    }
}

#[tool_handler]
impl ServerHandler for ProgressToolServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
    }
}

#[tokio::test]
async fn stateless_json_response_waits_for_terminal_tool_response() -> anyhow::Result<()> {
    let ct = CancellationToken::new();
    let service: StreamableHttpService<ProgressToolServer, LocalSessionManager> =
        StreamableHttpService::new(
            || Ok(ProgressToolServer::new()),
            Default::default(),
            StreamableHttpServerConfig {
                stateful_mode: false,
                json_response: true,
                sse_keep_alive: None,
                cancellation_token: ct.child_token(),
                ..Default::default()
            },
        );

    let router = axum::Router::new().nest_service("/mcp", service);
    let tcp_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = tcp_listener.local_addr()?;

    let handle = tokio::spawn({
        let ct = ct.clone();
        async move {
            let _ = axum::serve(tcp_listener, router)
                .with_graceful_shutdown(async move { ct.cancelled_owned().await })
                .await;
        }
    });

    let transport = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(format!("http://{addr}/mcp")),
    );
    let client = ().serve(transport).await?;

    let result = tokio::time::timeout(
        Duration::from_secs(3),
        client.call_tool(CallToolRequestParams::new("progress_then_result")),
    )
    .await??;

    let text = result
        .content
        .first()
        .and_then(|content| content.raw.as_text())
        .map(|text| text.text.as_str());
    assert_eq!(text, Some("done"));

    let _ = client.cancel().await;
    ct.cancel();
    handle.await?;
    Ok(())
}

#[tokio::test]
async fn stateless_sse_mode_default_unchanged() -> anyhow::Result<()> {
    let ct = CancellationToken::new();
    let (client, url, ct) = spawn_server(StreamableHttpServerConfig {
        stateful_mode: false,
        json_response: false,
        sse_keep_alive: None,
        cancellation_token: ct.child_token(),
        ..Default::default()
    })
    .await;

    let response = client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .body(INIT_BODY)
        .send()
        .await?;

    assert_eq!(response.status(), 200);

    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        content_type.contains("text/event-stream"),
        "Expected text/event-stream, got: {content_type}"
    );

    let body = response.text().await?;
    assert!(
        body.contains("data:"),
        "Expected SSE framing (data: prefix), got: {body}"
    );

    ct.cancel();
    Ok(())
}

#[tokio::test]
async fn json_response_ignored_in_stateful_mode() -> anyhow::Result<()> {
    let ct = CancellationToken::new();
    // json_response: true has no effect when stateful_mode: true — server still uses SSE
    let (client, url, ct) = spawn_server(StreamableHttpServerConfig {
        stateful_mode: true,
        json_response: true,
        sse_keep_alive: None,
        cancellation_token: ct.child_token(),
        ..Default::default()
    })
    .await;

    let response = client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .body(INIT_BODY)
        .send()
        .await?;

    assert_eq!(response.status(), 200);

    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        content_type.contains("text/event-stream"),
        "Stateful mode should always use SSE regardless of json_response, got: {content_type}"
    );

    ct.cancel();
    Ok(())
}
