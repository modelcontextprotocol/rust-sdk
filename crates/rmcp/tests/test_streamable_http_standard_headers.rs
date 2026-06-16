#![cfg(not(feature = "local"))]
//! SEP-2243 server-side validation of `Mcp-Method` / `Mcp-Name` headers.
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use tokio_util::sync::CancellationToken;

mod common;
use common::calculator::Calculator;

const SEP_VERSION: &str = "2026-07-28";

fn tools_call_body() -> String {
    r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"sum","arguments":{"a":1,"b":2}}}"#
        .to_owned()
}

async fn spawn_server() -> (reqwest::Client, String, CancellationToken) {
    let config = StreamableHttpServerConfig::default()
        .with_stateful_mode(false)
        .with_json_response(true)
        .with_sse_keep_alive(None)
        .with_cancellation_token(CancellationToken::new());
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
    (client, format!("http://{addr}/mcp"), ct)
}

/// POSTs a `tools/call` with the given protocol-version and optional SEP-2243 headers.
async fn post_tools_call(
    client: &reqwest::Client,
    url: &str,
    version: &str,
    mcp_method: Option<&str>,
    mcp_name: Option<&str>,
) -> reqwest::Response {
    let mut req = client
        .post(url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", version)
        .body(tools_call_body());
    if let Some(method) = mcp_method {
        req = req.header("Mcp-Method", method);
    }
    if let Some(name) = mcp_name {
        req = req.header("Mcp-Name", name);
    }
    req.send().await.expect("send tools/call request")
}

#[tokio::test]
async fn accepts_matching_standard_headers() -> anyhow::Result<()> {
    let (client, url, ct) = spawn_server().await;

    // Matching headers pass validation and reach dispatch. (Stateless mode without a
    // prior initialize yields an unrelated -32601, which still proves -32001 was not raised.)
    let response =
        post_tools_call(&client, &url, SEP_VERSION, Some("tools/call"), Some("sum")).await;
    let body: serde_json::Value = response.json().await?;
    assert_ne!(
        body["error"]["code"], -32001,
        "matching headers must not be rejected as a header mismatch, got: {body}"
    );

    ct.cancel();
    Ok(())
}

#[tokio::test]
async fn rejects_method_mismatch_with_32001() -> anyhow::Result<()> {
    let (client, url, ct) = spawn_server().await;

    let response =
        post_tools_call(&client, &url, SEP_VERSION, Some("tools/list"), Some("sum")).await;
    assert_eq!(response.status(), 400);
    let body: serde_json::Value = response.json().await?;
    assert_eq!(body["error"]["code"], -32001);

    ct.cancel();
    Ok(())
}

#[tokio::test]
async fn rejects_missing_method_header_with_32001() -> anyhow::Result<()> {
    let (client, url, ct) = spawn_server().await;

    let response = post_tools_call(&client, &url, SEP_VERSION, None, Some("sum")).await;
    assert_eq!(response.status(), 400);
    let body: serde_json::Value = response.json().await?;
    assert_eq!(body["error"]["code"], -32001);

    ct.cancel();
    Ok(())
}

#[tokio::test]
async fn rejects_name_mismatch_with_32001() -> anyhow::Result<()> {
    let (client, url, ct) = spawn_server().await;

    let response = post_tools_call(
        &client,
        &url,
        SEP_VERSION,
        Some("tools/call"),
        Some("product"),
    )
    .await;
    assert_eq!(response.status(), 400);
    let body: serde_json::Value = response.json().await?;
    assert_eq!(body["error"]["code"], -32001);

    ct.cancel();
    Ok(())
}

#[tokio::test]
async fn skips_validation_for_pre_sep_version() -> anyhow::Result<()> {
    let (client, url, ct) = spawn_server().await;

    // Older version: headers are not enforced even when absent.
    let response = post_tools_call(&client, &url, "2025-11-25", None, None).await;
    let body: serde_json::Value = response.json().await?;
    assert_ne!(
        body["error"]["code"], -32001,
        "pre-SEP versions must skip header validation, got: {body}"
    );

    ct.cancel();
    Ok(())
}
