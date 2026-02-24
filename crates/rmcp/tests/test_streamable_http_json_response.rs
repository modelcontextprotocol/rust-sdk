use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
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
