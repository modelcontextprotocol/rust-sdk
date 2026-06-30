#![cfg(not(feature = "local"))]
//! SEP-2243 server-side validation of `Mcp-Method` / `Mcp-Name` / `Mcp-Param-*` headers.
use std::sync::Arc;

use rmcp::{
    ServerHandler,
    model::{ServerCapabilities, ServerInfo, Tool},
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
};
use tokio_util::sync::CancellationToken;

const SEP_VERSION: &str = "2026-07-28";

/// Server exposing one tool whose `region` argument is promoted to `Mcp-Param-Region`.
#[derive(Clone, Default)]
struct HeaderValidationServer;

impl ServerHandler for HeaderValidationServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        if name != "deploy" {
            return None;
        }
        let schema = serde_json::json!({
            "type": "object",
            "properties": { "region": { "type": "string", "x-mcp-header": "Region" } }
        });
        let schema = schema.as_object().expect("object schema").clone();
        Some(Tool::new("deploy", "deploy a thing", Arc::new(schema)))
    }
}

async fn spawn_server() -> (reqwest::Client, String, CancellationToken) {
    let config = StreamableHttpServerConfig::default()
        .with_stateful_mode(false)
        .with_json_response(true)
        .with_sse_keep_alive(None)
        .with_cancellation_token(CancellationToken::new());
    let ct = config.cancellation_token.clone();
    let service: StreamableHttpService<HeaderValidationServer, LocalSessionManager> =
        StreamableHttpService::new(|| Ok(HeaderValidationServer), Default::default(), config);

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
    (reqwest::Client::new(), format!("http://{addr}/mcp"), ct)
}

/// POSTs a `tools/call` with the given protocol-version and optional SEP-2243 headers.
async fn post_tool_call(
    client: &reqwest::Client,
    url: &str,
    version: &str,
    tool_name: &str,
    arguments: serde_json::Value,
    mcp_method: Option<&str>,
    mcp_name: Option<&str>,
    param_region: Option<&str>,
) -> reqwest::Response {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": tool_name,
            "arguments": arguments,
        }
    });
    let mut req = client
        .post(url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", version)
        .body(body.to_string());
    if let Some(method) = mcp_method {
        req = req.header("Mcp-Method", method);
    }
    if let Some(name) = mcp_name {
        req = req.header("Mcp-Name", name);
    }
    if let Some(region) = param_region {
        req = req.header("Mcp-Param-Region", region);
    }
    req.send().await.expect("send tools/call request")
}

#[tokio::test]
async fn accepts_matching_standard_headers() -> anyhow::Result<()> {
    let (client, url, ct) = spawn_server().await;

    // Matching headers pass validation and reach dispatch. (Stateless mode without a
    // prior initialize yields an unrelated -32601, which still proves -32020 was not raised.)
    let response = post_tool_call(
        &client,
        &url,
        SEP_VERSION,
        "sum",
        serde_json::json!({ "a": 1, "b": 2 }),
        Some("tools/call"),
        Some("sum"),
        None,
    )
    .await;
    let body: serde_json::Value = response.json().await?;
    assert_ne!(
        body["error"]["code"], -32020,
        "matching headers must not be rejected as a header mismatch, got: {body}"
    );

    ct.cancel();
    Ok(())
}

#[tokio::test]
async fn rejects_method_mismatch_with_32020() -> anyhow::Result<()> {
    let (client, url, ct) = spawn_server().await;

    let response = post_tool_call(
        &client,
        &url,
        SEP_VERSION,
        "sum",
        serde_json::json!({ "a": 1, "b": 2 }),
        Some("tools/list"),
        Some("sum"),
        None,
    )
    .await;
    assert_eq!(response.status(), 400);
    let body: serde_json::Value = response.json().await?;
    assert_eq!(body["error"]["code"], -32020);

    ct.cancel();
    Ok(())
}

#[tokio::test]
async fn rejects_missing_method_header_with_32020() -> anyhow::Result<()> {
    let (client, url, ct) = spawn_server().await;

    let response = post_tool_call(
        &client,
        &url,
        SEP_VERSION,
        "sum",
        serde_json::json!({ "a": 1, "b": 2 }),
        None,
        Some("sum"),
        None,
    )
    .await;
    assert_eq!(response.status(), 400);
    let body: serde_json::Value = response.json().await?;
    assert_eq!(body["error"]["code"], -32020);

    ct.cancel();
    Ok(())
}

#[tokio::test]
async fn rejects_name_mismatch_with_32020() -> anyhow::Result<()> {
    let (client, url, ct) = spawn_server().await;

    let response = post_tool_call(
        &client,
        &url,
        SEP_VERSION,
        "sum",
        serde_json::json!({ "a": 1, "b": 2 }),
        Some("tools/call"),
        Some("product"),
        None,
    )
    .await;
    assert_eq!(response.status(), 400);
    let body: serde_json::Value = response.json().await?;
    assert_eq!(body["error"]["code"], -32020);

    ct.cancel();
    Ok(())
}

#[tokio::test]
async fn skips_validation_for_pre_sep_version() -> anyhow::Result<()> {
    let (client, url, ct) = spawn_server().await;

    // Older version: headers are not enforced even when absent.
    let response = post_tool_call(
        &client,
        &url,
        "2025-11-25",
        "sum",
        serde_json::json!({ "a": 1, "b": 2 }),
        None,
        None,
        None,
    )
    .await;
    let body: serde_json::Value = response.json().await?;
    assert_ne!(
        body["error"]["code"], -32020,
        "pre-SEP versions must skip header validation, got: {body}"
    );

    ct.cancel();
    Ok(())
}

#[tokio::test]
async fn accepts_matching_param_header() -> anyhow::Result<()> {
    let (client, url, ct) = spawn_server().await;

    let response = post_tool_call(
        &client,
        &url,
        SEP_VERSION,
        "deploy",
        serde_json::json!({ "region": "us-west1" }),
        Some("tools/call"),
        Some("deploy"),
        Some("us-west1"),
    )
    .await;
    let body: serde_json::Value = response.json().await?;
    assert_ne!(
        body["error"]["code"], -32020,
        "matching Mcp-Param-* must not be rejected, got: {body}"
    );

    ct.cancel();
    Ok(())
}

#[tokio::test]
async fn rejects_param_mismatch_with_32020() -> anyhow::Result<()> {
    let (client, url, ct) = spawn_server().await;

    let response = post_tool_call(
        &client,
        &url,
        SEP_VERSION,
        "deploy",
        serde_json::json!({ "region": "us-west1" }),
        Some("tools/call"),
        Some("deploy"),
        Some("eu-central1"),
    )
    .await;
    assert_eq!(response.status(), 400);
    let body: serde_json::Value = response.json().await?;
    assert_eq!(body["error"]["code"], -32020);

    ct.cancel();
    Ok(())
}

#[tokio::test]
async fn rejects_missing_param_header_with_32020() -> anyhow::Result<()> {
    let (client, url, ct) = spawn_server().await;

    // `region` argument is present but the annotated `Mcp-Param-Region` header is absent.
    let response = post_tool_call(
        &client,
        &url,
        SEP_VERSION,
        "deploy",
        serde_json::json!({ "region": "us-west1" }),
        Some("tools/call"),
        Some("deploy"),
        None,
    )
    .await;
    assert_eq!(response.status(), 400);
    let body: serde_json::Value = response.json().await?;
    assert_eq!(body["error"]["code"], -32020);

    ct.cancel();
    Ok(())
}
