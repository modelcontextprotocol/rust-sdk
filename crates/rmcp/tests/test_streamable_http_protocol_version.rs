#![cfg(not(feature = "local"))]
//! Regression tests for the `MCP-Protocol-Version` header / initialize body consistency check.
use std::sync::Arc;

use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use tokio_util::sync::CancellationToken;

mod common;
use common::calculator::Calculator;

fn init_body(body_version: &str) -> String {
    format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"protocolVersion":"{body_version}","capabilities":{{}},"clientInfo":{{"name":"test","version":"1.0"}}}}}}"#
    )
}

async fn spawn_server(
    config: StreamableHttpServerConfig,
) -> (reqwest::Client, String, CancellationToken) {
    spawn_server_with_manager(config, Arc::new(LocalSessionManager::default())).await
}

async fn spawn_server_with_manager(
    config: StreamableHttpServerConfig,
    session_manager: Arc<LocalSessionManager>,
) -> (reqwest::Client, String, CancellationToken) {
    let ct = config.cancellation_token.clone();
    let service: StreamableHttpService<Calculator, LocalSessionManager> =
        StreamableHttpService::new(|| Ok(Calculator::new()), session_manager, config);

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

fn stateless_json_config() -> StreamableHttpServerConfig {
    StreamableHttpServerConfig::default()
        .with_stateful_mode(false)
        .with_json_response(true)
        .with_sse_keep_alive(None)
        .with_cancellation_token(CancellationToken::new())
}

fn stateful_config() -> StreamableHttpServerConfig {
    StreamableHttpServerConfig::default()
        .with_stateful_mode(true)
        .with_sse_keep_alive(None)
        .with_cancellation_token(CancellationToken::new())
}

async fn post_init(
    client: &reqwest::Client,
    url: &str,
    header: Option<&str>,
    body_version: &str,
) -> reqwest::Response {
    let mut req = client
        .post(url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .body(init_body(body_version));
    if let Some(h) = header {
        req = req.header("MCP-Protocol-Version", h);
    }
    req.send().await.expect("send initialize request")
}

async fn post_non_initialize(client: &reqwest::Client, url: &str) -> reqwest::Response {
    client
        .post(url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .body(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#)
        .send()
        .await
        .expect("send non-initialize request")
}

#[tokio::test]
async fn stateless_init_rejects_when_header_older_than_body() -> anyhow::Result<()> {
    let (client, url, ct) = spawn_server(stateless_json_config()).await;

    let response = post_init(&client, &url, Some("2025-03-26"), "2025-11-25").await;
    assert_eq!(response.status(), 400);

    let body: serde_json::Value = response.json().await?;
    assert_eq!(body["error"]["code"], -32600);
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("MCP-Protocol-Version"),
        "expected error message to mention the header, got: {body}"
    );

    ct.cancel();
    Ok(())
}

#[tokio::test]
async fn stateless_init_rejects_when_header_newer_than_body() -> anyhow::Result<()> {
    let (client, url, ct) = spawn_server(stateless_json_config()).await;

    let response = post_init(&client, &url, Some("2025-11-25"), "2025-03-26").await;
    assert_eq!(response.status(), 400);

    let body: serde_json::Value = response.json().await?;
    assert_eq!(body["error"]["code"], -32600);

    ct.cancel();
    Ok(())
}

#[tokio::test]
async fn stateless_init_accepts_when_header_matches_body() -> anyhow::Result<()> {
    let (client, url, ct) = spawn_server(stateless_json_config()).await;

    let response = post_init(&client, &url, Some("2025-11-25"), "2025-11-25").await;
    assert_eq!(response.status(), 200);

    let body: serde_json::Value = response.json().await?;
    assert!(
        body["result"].is_object(),
        "expected an InitializeResult, got: {body}"
    );

    ct.cancel();
    Ok(())
}

#[tokio::test]
async fn stateless_init_accepts_when_header_absent() -> anyhow::Result<()> {
    let (client, url, ct) = spawn_server(stateless_json_config()).await;

    let response = post_init(&client, &url, None, "2025-11-25").await;
    assert_eq!(response.status(), 200);

    ct.cancel();
    Ok(())
}

#[tokio::test]
async fn stateful_init_rejects_when_header_mismatches_body() -> anyhow::Result<()> {
    let (client, url, ct) = spawn_server(stateful_config()).await;

    let response = post_init(&client, &url, Some("2024-11-05"), "2025-11-25").await;
    assert_eq!(response.status(), 400);

    let body: serde_json::Value = response.json().await?;
    assert_eq!(body["error"]["code"], -32600);

    ct.cancel();
    Ok(())
}

#[tokio::test]
async fn stateful_rejected_initial_posts_do_not_create_sessions() -> anyhow::Result<()> {
    let session_manager = Arc::new(LocalSessionManager::default());
    let (client, url, ct) =
        spawn_server_with_manager(stateful_config(), session_manager.clone()).await;

    let response = post_non_initialize(&client, &url).await;
    assert_eq!(response.status(), 422);
    assert_eq!(session_manager.sessions.read().await.len(), 0);

    let response = post_init(&client, &url, Some("2024-11-05"), "2025-11-25").await;
    assert_eq!(response.status(), 400);
    assert_eq!(session_manager.sessions.read().await.len(), 0);

    ct.cancel();
    Ok(())
}
