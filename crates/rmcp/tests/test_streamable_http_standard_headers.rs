#![cfg(not(feature = "local"))]
//! SEP-2243 server-side validation of `Mcp-Method` / `Mcp-Name` / `Mcp-Param-*` headers.
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use rmcp::{
    ServerHandler,
    model::{ServerCapabilities, ServerConfig, Tool},
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
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
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
    spawn_server_with(|| Ok(HeaderValidationServer)).await
}

async fn spawn_server_with<S>(
    service_factory: impl Fn() -> Result<S, std::io::Error> + Send + Sync + 'static,
) -> (reqwest::Client, String, CancellationToken)
where
    S: ServerHandler + Send + 'static,
{
    spawn_server_with_mode(service_factory, false).await
}

async fn spawn_server_with_mode<S>(
    service_factory: impl Fn() -> Result<S, std::io::Error> + Send + Sync + 'static,
    legacy_session_mode: bool,
) -> (reqwest::Client, String, CancellationToken)
where
    S: ServerHandler + Send + 'static,
{
    let config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(legacy_session_mode)
        .with_json_response(true)
        .with_sse_keep_alive(None)
        .with_cancellation_token(CancellationToken::new());
    let ct = config.cancellation_token.clone();
    let service: StreamableHttpService<S, LocalSessionManager> =
        StreamableHttpService::new(service_factory, Default::default(), config);

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
async fn rejects_decoded_base64_param_mismatch_with_32020() -> anyhow::Result<()> {
    let (client, url, ct) = spawn_server().await;

    let response = post_tool_call(
        &client,
        &url,
        SEP_VERSION,
        "deploy",
        serde_json::json!({ "region": "us-west1" }),
        Some("tools/call"),
        Some("deploy"),
        Some("=?base64?ZXUtY2VudHJhbDE=?="),
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

/// Server whose `deploy` tool gains or loses the `x-mcp-header` annotation at runtime.
#[derive(Clone, Default)]
struct ChangingToolServer {
    annotated: Arc<AtomicBool>,
}

async fn spawn_changing_tool_server(
    server: &ChangingToolServer,
) -> (reqwest::Client, String, CancellationToken) {
    let server = server.clone();
    spawn_server_with(move || Ok(server.clone())).await
}

/// POSTs a `deploy` call whose `Mcp-Param-Region` header contradicts the body.
async fn post_mismatched_region(client: &reqwest::Client, url: &str) -> reqwest::Response {
    post_tool_call(
        client,
        url,
        SEP_VERSION,
        "deploy",
        serde_json::json!({ "region": "us-west1" }),
        Some("tools/call"),
        Some("deploy"),
        Some("eu-central1"),
    )
    .await
}

impl ServerHandler for ChangingToolServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        if !self.annotated.load(Ordering::SeqCst) {
            return None;
        }
        HeaderValidationServer.get_tool(name)
    }
}

#[tokio::test]
async fn validates_param_header_once_tool_gains_annotation() -> anyhow::Result<()> {
    let server = ChangingToolServer::default();
    let (client, url, ct) = spawn_changing_tool_server(&server).await;

    // Header validation passes, so the request fails later on its missing `_meta`.
    let response = post_mismatched_region(&client, &url).await;
    assert_eq!(response.status(), 400);
    let body: serde_json::Value = response.json().await?;
    assert_eq!(body["error"]["code"], -32602);

    server.annotated.store(true, Ordering::SeqCst);
    let response = post_mismatched_region(&client, &url).await;
    assert_eq!(response.status(), 400);
    let body: serde_json::Value = response.json().await?;
    assert_eq!(body["error"]["code"], -32020);

    ct.cancel();
    Ok(())
}

#[tokio::test]
async fn skips_param_header_validation_once_tool_loses_annotation() -> anyhow::Result<()> {
    let server = ChangingToolServer {
        annotated: Arc::new(AtomicBool::new(true)),
    };
    let (client, url, ct) = spawn_changing_tool_server(&server).await;

    let response = post_mismatched_region(&client, &url).await;
    assert_eq!(response.status(), 400);
    let body: serde_json::Value = response.json().await?;
    assert_eq!(body["error"]["code"], -32020);

    server.annotated.store(false, Ordering::SeqCst);
    let response = post_mismatched_region(&client, &url).await;
    assert_eq!(response.status(), 400);
    let body: serde_json::Value = response.json().await?;
    assert_eq!(body["error"]["code"], -32602);

    ct.cancel();
    Ok(())
}

/// Spawns the server in legacy session mode: the initialize handshake creates
/// a real session, exercising the path where the validator used to be bypassed.
async fn spawn_legacy_server() -> (reqwest::Client, String, CancellationToken) {
    let config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(true)
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

/// POSTs an `initialize` request with the given protocol version and optional
/// `Mcp-Method` header.
async fn post_initialize(
    client: &reqwest::Client,
    url: &str,
    version: &str,
    mcp_method: Option<&str>,
) -> reqwest::Response {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": version,
            "capabilities": {},
            "clientInfo": { "name": "test", "version": "1.0" },
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
    req.send().await.expect("send initialize request")
}

#[tokio::test]
async fn rejects_initialize_with_contradicting_mcp_method_before_session_creation()
-> anyhow::Result<()> {
    let (client, url, ct) = spawn_legacy_server().await;

    // Regression test: in legacy session mode with no session id, an
    // `initialize` body carrying a contradictory `Mcp-Method` header used to
    // bypass the validator, returning HTTP 200 and creating a session.
    let response = post_initialize(&client, &url, SEP_VERSION, Some("tools/list")).await;
    assert_eq!(response.status(), 400);
    let body: serde_json::Value = response.json().await?;
    assert_eq!(body["error"]["code"], -32020);

    ct.cancel();
    Ok(())
}

#[tokio::test]
async fn accepts_initialize_with_matching_mcp_method() -> anyhow::Result<()> {
    let (client, url, ct) = spawn_legacy_server().await;

    // A matching Mcp-Method header on initialize passes validation and the
    // handshake completes (HTTP 200, session created).
    let response = post_initialize(&client, &url, SEP_VERSION, Some("initialize")).await;
    assert_eq!(response.status(), 200);

    ct.cancel();
    Ok(())
}

#[tokio::test]
async fn media_type_headers_follow_http_semantics() {
    let (client, url, ct) = spawn_server().await;
    let cases: &[(&str, &[&str], &str, u16)] = &[
        (
            "ordinary",
            &["application/json, text/event-stream"],
            "application/json",
            202,
        ),
        (
            "content type case",
            &["application/json, text/event-stream"],
            "Application/JSON",
            202,
        ),
        (
            "accept case",
            &["APPLICATION/JSON, TEXT/EVENT-STREAM"],
            "application/json",
            202,
        ),
        (
            "split accept",
            &["application/json", "text/event-stream"],
            "application/json",
            202,
        ),
        (
            "content type suffix",
            &["application/json, text/event-stream"],
            "application/jsonx",
            415,
        ),
        (
            "accept suffix",
            &["application/json-seq, text/event-stream"],
            "application/json",
            406,
        ),
        (
            "zero quality",
            &["application/json;q=0, text/event-stream"],
            "application/json",
            406,
        ),
        (
            "zero quality decimal",
            &["application/json;Q=0.000, text/event-stream"],
            "application/json",
            406,
        ),
        (
            "positive quality",
            &["application/json;q=0.001, text/event-stream"],
            "application/json",
            202,
        ),
        (
            "parameters",
            &["application/json;profile=\"a,b;c\", text/event-stream"],
            "application/json; charset=utf-8",
            202,
        ),
        (
            "parameter is not media",
            &["text/plain;profile=\"application/json, text/event-stream\""],
            "application/json",
            406,
        ),
        (
            "escaped quoted parameter",
            &[r#"application/json;profile="a\",b;c", text/event-stream"#],
            "application/json",
            202,
        ),
        (
            "quoted quality is not a weight",
            &[r#"application/json;profile="a;q=0", text/event-stream"#],
            "application/json",
            202,
        ),
        (
            "escaped parameter is not media",
            &[r#"text/plain;profile="a\",application/json, text/event-stream""#],
            "application/json",
            406,
        ),
        ("wildcard alone", &["*/*"], "application/json", 406),
        // Invalid weights follow this implementation's admission policy.
        (
            "one",
            &["application/json;q=1, text/event-stream"],
            "application/json",
            202,
        ),
        (
            "one decimal",
            &["application/json;q=1., text/event-stream"],
            "application/json",
            202,
        ),
        (
            "one three digits",
            &["application/json;q=1.000, text/event-stream"],
            "application/json",
            202,
        ),
        (
            "over precision",
            &["application/json;q=0.0001, text/event-stream"],
            "application/json",
            406,
        ),
        (
            "above one",
            &["application/json;q=1.001, text/event-stream"],
            "application/json",
            406,
        ),
        (
            "missing leading zero",
            &["application/json;q=.5, text/event-stream"],
            "application/json",
            406,
        ),
        (
            "negative quality",
            &["application/json;q=-0.1, text/event-stream"],
            "application/json",
            406,
        ),
        (
            "non digit quality",
            &["application/json;q=abc, text/event-stream"],
            "application/json",
            406,
        ),
        (
            "zero SSE",
            &["application/json, text/event-stream;q=0"],
            "application/json",
            406,
        ),
        (
            "positive SSE",
            &["application/json, text/event-stream;q=0.01"],
            "application/json",
            202,
        ),
        (
            "weighted split reversed",
            &["text/event-stream;q=0.5", "application/json;q=0.5"],
            "application/json",
            202,
        ),
        (
            "same field reversed",
            &["text/event-stream, application/json"],
            "application/json",
            202,
        ),
        (
            "SSE near match",
            &["application/json, text/event-streaming"],
            "application/json",
            406,
        ),
    ];
    let mut failures = Vec::new();
    for (name, accept, content_type, expected) in cases {
        let mut request = client
            .post(&url)
            .header("Content-Type", *content_type)
            .body(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#);
        for value in *accept {
            request = request.header("Accept", *value);
        }
        let actual = request.send().await.unwrap().status().as_u16();
        if actual != *expected {
            failures.push(format!("{name}: expected {expected}, got {actual}"));
        }
    }
    ct.cancel();
    // Legacy mode dispatches GET through media validation before requiring a session ID.
    let (client, url, ct) = spawn_server_with_mode(|| Ok(HeaderValidationServer), true).await;
    for (accept, expected) in [
        ("text/event-stream", 400),
        ("TEXT/EVENT-STREAM", 400),
        ("text/event-streamx", 406),
        ("text/event-stream;q=0.0", 406),
        ("text/event-stream;q=1", 400),
        ("text/event-stream;q=1.", 400),
        ("text/event-stream;q=1.000", 400),
        ("text/event-stream;q=1.001", 406),
        ("text/event-stream;q=0.0001", 406),
        ("text/event-stream;q=.5", 406),
        ("text/event-stream;q=-0.1", 406),
        ("text/event-stream;q=abc", 406),
    ] {
        let actual = client
            .get(&url)
            .header("Accept", accept)
            .send()
            .await
            .unwrap()
            .status()
            .as_u16();
        if actual != expected {
            failures.push(format!("GET {accept}: expected {expected}, got {actual}"));
        }
    }
    ct.cancel();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
