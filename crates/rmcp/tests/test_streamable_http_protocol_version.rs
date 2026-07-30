#![cfg(not(feature = "local"))]
//! Streamable HTTP protocol-version and request-metadata validation tests.
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use rmcp::{
    ServerHandler,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
};
use serde_json::{Value, json};
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
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_sse_keep_alive(None)
        .with_cancellation_token(CancellationToken::new())
}

fn stateful_config() -> StreamableHttpServerConfig {
    StreamableHttpServerConfig::default()
        .with_legacy_session_mode(true)
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

async fn post_modern_request(
    client: &reqwest::Client,
    url: &str,
    method: &str,
    name: Option<&str>,
    params: Value,
) -> reqwest::Response {
    let mut request = client
        .post(url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", method)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params,
        }));
    if let Some(name) = name {
        request = request.header("Mcp-Name", name);
    }
    request.send().await.expect("send modern request")
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

#[tokio::test]
async fn stateless_missing_protocol_header_returns_header_mismatch() -> anyhow::Result<()> {
    let (client, url, ct) = spawn_server(stateless_json_config()).await;

    let body = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientInfo":{"name":"test","version":"1.0"},"io.modelcontextprotocol/clientCapabilities":{}}}}"#;
    let response = client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("Mcp-Method", "tools/list")
        .body(body)
        .send()
        .await
        .expect("send non-initialize request");

    assert_eq!(response.status(), 400);

    let body: serde_json::Value = response.json().await?;
    assert_eq!(body["jsonrpc"], "2.0");
    assert_eq!(body["id"], 1);
    assert_eq!(body["error"]["code"], -32020);
    assert!(
        body["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("requires MCP-Protocol-Version header")),
        "expected missing protocol header message, got: {body}"
    );

    ct.cancel();
    Ok(())
}

#[tokio::test]
async fn stateless_tools_list_rejects_missing_request_meta() {
    let (client, url, ct) = spawn_server(stateless_json_config()).await;

    let response = post_modern_request(&client, &url, "tools/list", None, json!({})).await;

    assert_eq!(response.status(), 400);
    let body: Value = response.json().await.expect("response should be JSON");
    assert_eq!(body["error"]["code"], -32602);
    assert!(
        body["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("io.modelcontextprotocol/protocolVersion")),
        "expected error message to mention protocolVersion, got: {body}"
    );
    ct.cancel();
}

#[tokio::test]
async fn stateless_tools_call_rejects_missing_request_meta() {
    let (client, url, ct) = spawn_server(stateless_json_config()).await;

    let response = post_modern_request(
        &client,
        &url,
        "tools/call",
        Some("sum"),
        json!({
            "name": "sum",
            "arguments": {
                "a": 1,
                "b": 2
            }
        }),
    )
    .await;

    assert_eq!(response.status(), 400);
    let body: Value = response.json().await.expect("response should be JSON");
    assert_eq!(body["error"]["code"], -32602);
    assert!(
        body["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("io.modelcontextprotocol/protocolVersion")),
        "expected error message to mention protocolVersion, got: {body}"
    );
    ct.cancel();
}

#[tokio::test]
async fn stateless_request_rejects_missing_meta_protocol_version() {
    let (client, url, ct) = spawn_server(stateless_json_config()).await;

    let response = post_modern_request(
        &client,
        &url,
        "tools/list",
        None,
        json!({
            "_meta": {
                "io.modelcontextprotocol/clientInfo": {
                    "name": "test",
                    "version": "1.0"
                },
                "io.modelcontextprotocol/clientCapabilities": {}
            }
        }),
    )
    .await;

    assert_eq!(response.status(), 400);
    let body: Value = response.json().await.expect("response should be JSON");
    assert_eq!(body["error"]["code"], -32602);
    assert!(
        body["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("io.modelcontextprotocol/protocolVersion")),
        "expected error message to mention protocolVersion, got: {body}"
    );
    ct.cancel();
}

#[tokio::test]
async fn stateless_request_rejects_missing_meta_client_capabilities() {
    let (client, url, ct) = spawn_server(stateless_json_config()).await;

    let response = post_modern_request(
        &client,
        &url,
        "tools/list",
        None,
        json!({
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                "io.modelcontextprotocol/clientInfo": {
                    "name": "test",
                    "version": "1.0"
                }
            }
        }),
    )
    .await;

    assert_eq!(response.status(), 400);
    let body: Value = response.json().await.expect("response should be JSON");
    assert_eq!(body["error"]["code"], -32602);
    assert!(
        body["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("io.modelcontextprotocol/clientCapabilities")),
        "expected error message to mention clientCapabilities, got: {body}"
    );
    ct.cancel();
}

#[tokio::test]
async fn stateless_request_accepts_missing_optional_meta_client_info() {
    let (client, url, ct) = spawn_server(stateless_json_config()).await;

    let response = post_modern_request(
        &client,
        &url,
        "tools/list",
        None,
        json!({
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                "io.modelcontextprotocol/clientCapabilities": {}
            }
        }),
    )
    .await;

    assert_eq!(response.status(), 200);
    ct.cancel();
}

// ---------------------------------------------------------------------------
// Opt-in seam: `with_stateless_protocol_metadata_required(true)`
// ---------------------------------------------------------------------------
// In stateless mode, every non-initialize Streamable HTTP JSON-RPC request
// POST must carry the `MCP-Protocol-Version` HTTP header (missing → HTTP 400 /
// JSON-RPC `-32020` HeaderMismatch before dispatch). Every non-initialize,
// non-discover request must additionally carry a per-request
// `io.modelcontextprotocol/protocolVersion` in `_meta` (missing → HTTP 400 /
// JSON-RPC `-32602`). The existing rule that requires `server/discover` to
// carry `_meta.protocolVersion` is preserved unchanged.
//
// Non-dispatch is proven with an explicit invocation counter (`AtomicUsize`).

#[derive(Clone)]
struct CountingServer {
    lists: Arc<AtomicUsize>,
}

impl CountingServer {
    fn new() -> (Self, Arc<AtomicUsize>) {
        let lists = Arc::new(AtomicUsize::new(0));
        (
            Self {
                lists: lists.clone(),
            },
            lists,
        )
    }
}

impl ServerHandler for CountingServer {
    fn get_info(&self) -> rmcp::model::ServerInfo {
        rmcp::model::ServerInfo::new(
            rmcp::model::ServerCapabilities::builder()
                .enable_tools()
                .build(),
        )
    }

    fn list_tools(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParams>,
        _context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> impl std::future::Future<Output = Result<rmcp::model::ListToolsResult, rmcp::ErrorData>>
    + Send
    + '_ {
        self.lists.fetch_add(1, Ordering::SeqCst);
        std::future::ready(Ok(rmcp::model::ListToolsResult::default()))
    }
}

async fn spawn_counting(
    config: StreamableHttpServerConfig,
) -> (reqwest::Client, String, CancellationToken, Arc<AtomicUsize>) {
    let ct = config.cancellation_token.clone();
    let (server, lists) = CountingServer::new();
    let service: StreamableHttpService<CountingServer, LocalSessionManager> =
        StreamableHttpService::new(move || Ok(server.clone()), Default::default(), config);

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

    (
        reqwest::Client::new(),
        format!("http://{addr}/mcp"),
        ct,
        lists,
    )
}

fn modern_required_config() -> StreamableHttpServerConfig {
    StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_sse_keep_alive(None)
        .with_stateless_protocol_metadata_required(true)
        .with_cancellation_token(CancellationToken::new())
}

async fn post_seam(
    client: &reqwest::Client,
    url: &str,
    body: &str,
    header_version: Option<&str>,
    extra_headers: &[(&str, &str)],
) -> reqwest::Response {
    let mut req = client
        .post(url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .body(body.to_owned());
    if let Some(h) = header_version {
        req = req.header("MCP-Protocol-Version", h);
    }
    for (k, v) in extra_headers {
        req = req.header(*k, *v);
    }
    req.send().await.expect("send request")
}

// With the seam disabled, explicit stateless mode still dispatches a request
// without protocol metadata, preserving backwards compatibility.
#[tokio::test]
async fn seam_disabled_preserves_stateless_compatibility() -> anyhow::Result<()> {
    let (client, url, ct, lists) = spawn_counting(stateless_json_config()).await;

    let body = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#;
    let response = post_seam(&client, &url, body, None, &[]).await;
    assert_eq!(response.status(), 200);
    assert_eq!(
        lists.load(Ordering::SeqCst),
        1,
        "seam-disabled stateless config must dispatch exactly once"
    );

    ct.cancel();
    Ok(())
}

// With legacy compatibility enabled, the seam applies only to requests that
// are routed statelessly: an established legacy session remains unchanged,
// while a self-identifying modern request is still enforced.
#[tokio::test]
async fn seam_mixed_mode_enforces_only_stateless_routed_requests() -> anyhow::Result<()> {
    let config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(true)
        .with_json_response(true)
        .with_sse_keep_alive(None)
        .with_stateless_protocol_metadata_required(true)
        .with_cancellation_token(CancellationToken::new());
    let (client, url, ct, lists) = spawn_counting(config).await;

    let legacy_body = r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#;
    let unknown_response = post_seam(
        &client,
        &url,
        legacy_body,
        None,
        &[("Mcp-Session-Id", "unknown-session")],
    )
    .await;
    assert_eq!(
        unknown_response.status(),
        reqwest::StatusCode::NOT_FOUND,
        "the stateless seam must not shadow the legacy unknown-session boundary"
    );
    assert_eq!(lists.load(Ordering::SeqCst), 0);

    let initialize = post_init(&client, &url, None, "2025-11-25").await;
    assert_eq!(initialize.status(), 200);
    let session_id = initialize
        .headers()
        .get("Mcp-Session-Id")
        .and_then(|value| value.to_str().ok())
        .expect("legacy initialize response must include a session id")
        .to_owned();

    let legacy_response = post_seam(
        &client,
        &url,
        legacy_body,
        None,
        &[("Mcp-Session-Id", &session_id)],
    )
    .await;
    assert_eq!(legacy_response.status(), 200);
    let _ = legacy_response.bytes().await?;
    assert_eq!(
        lists.load(Ordering::SeqCst),
        1,
        "legacy session routing must remain unchanged"
    );

    let delete_response = client
        .delete(&url)
        .header("Mcp-Session-Id", &session_id)
        .send()
        .await?;
    assert_eq!(delete_response.status(), reqwest::StatusCode::ACCEPTED);

    let terminated_response = post_seam(
        &client,
        &url,
        legacy_body,
        None,
        &[("Mcp-Session-Id", &session_id)],
    )
    .await;
    assert_eq!(
        terminated_response.status(),
        reqwest::StatusCode::NOT_FOUND,
        "the stateless seam must not shadow the legacy terminated-session boundary"
    );
    assert_eq!(lists.load(Ordering::SeqCst), 1);

    let modern_body = r#"{"jsonrpc":"2.0","id":3,"method":"tools/list","params":{}}"#;
    let modern_response = post_seam(
        &client,
        &url,
        modern_body,
        Some("2026-07-28"),
        &[("Mcp-Method", "tools/list")],
    )
    .await;
    assert_eq!(modern_response.status(), 400);
    let payload: serde_json::Value = modern_response.json().await?;
    assert_eq!(payload["error"]["code"], -32602);
    assert_eq!(
        lists.load(Ordering::SeqCst),
        1,
        "stateless metadata rejection must happen before another handler dispatch"
    );

    ct.cancel();
    Ok(())
}

// Both signals absent → missing header / -32020 before dispatch.
#[tokio::test]
async fn seam_opt_in_rejects_missing_header_before_dispatch() -> anyhow::Result<()> {
    let (client, url, ct, lists) = spawn_counting(modern_required_config()).await;

    let without_meta = r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#;
    let response = post_seam(&client, &url, without_meta, None, &[]).await;
    assert_eq!(response.status(), 400, "missing header must yield HTTP 400");
    let payload: serde_json::Value = response.json().await?;
    assert_eq!(payload["error"]["code"], -32020);
    assert_eq!(
        lists.load(Ordering::SeqCst),
        0,
        "handler invocation counter must stay at 0 when the header is missing"
    );

    ct.cancel();
    Ok(())
}

// Header present but `_meta.protocolVersion` absent → -32602 before
// dispatch. Counter must equal 0.
#[tokio::test]
async fn seam_opt_in_rejects_missing_meta_before_dispatch() -> anyhow::Result<()> {
    let (client, url, ct, lists) = spawn_counting(modern_required_config()).await;

    let body = r#"{"jsonrpc":"2.0","id":4,"method":"tools/list","params":{}}"#;
    let response = post_seam(
        &client,
        &url,
        body,
        Some("2026-07-28"),
        &[("Mcp-Method", "tools/list")],
    )
    .await;
    assert_eq!(response.status(), 400);
    let payload: serde_json::Value = response.json().await?;
    assert_eq!(payload["error"]["code"], -32602);
    assert_eq!(
        lists.load(Ordering::SeqCst),
        0,
        "handler invocation counter must stay at 0 when _meta.protocolVersion is missing"
    );

    ct.cancel();
    Ok(())
}

// A 2026 request with protocolVersion but no clientCapabilities reaches the
// existing inline-metadata validator and is rejected before method dispatch.
#[tokio::test]
async fn seam_opt_in_rejects_missing_client_capabilities_before_dispatch() -> anyhow::Result<()> {
    let (client, url, ct, lists) = spawn_counting(modern_required_config()).await;

    let body = r#"{"jsonrpc":"2.0","id":5,"method":"tools/list","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28"}}}"#;
    let response = post_seam(
        &client,
        &url,
        body,
        Some("2026-07-28"),
        &[("Mcp-Method", "tools/list")],
    )
    .await;
    assert_eq!(response.status(), 400);
    let payload: serde_json::Value = response.json().await?;
    assert_eq!(payload["error"]["code"], -32602);
    assert_eq!(
        lists.load(Ordering::SeqCst),
        0,
        "handler invocation counter must stay at 0 when clientCapabilities is missing"
    );

    ct.cancel();
    Ok(())
}

// Both signals present at the current version, plus the routing header
// required by `validate_standard_headers` for `>= STANDARD_HEADERS`. The
// seam must dispatch and the handler must run exactly once.
#[tokio::test]
async fn seam_opt_in_dispatches_when_header_and_meta_present() -> anyhow::Result<()> {
    let (client, url, ct, lists) = spawn_counting(modern_required_config()).await;

    let body = r#"{"jsonrpc":"2.0","id":6,"method":"tools/list","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientInfo":{"name":"test","version":"1.0"},"io.modelcontextprotocol/clientCapabilities":{}}}}"#;
    let response = post_seam(
        &client,
        &url,
        body,
        Some("2026-07-28"),
        &[("Mcp-Method", "tools/list")],
    )
    .await;
    assert_eq!(response.status(), 200);
    let payload: serde_json::Value = response.json().await?;
    assert!(payload.get("result").is_some());
    assert_eq!(
        lists.load(Ordering::SeqCst),
        1,
        "handler invocation counter must equal 1 on successful dispatch"
    );

    ct.cancel();
    Ok(())
}

// rmcp clients negotiated below 2026-07-28 send the protocol header but do
// not attach per-request protocol metadata. Requiring the metadata therefore
// rejects their real request shape even if the handler supports that version.
#[tokio::test]
async fn seam_opt_in_rejects_older_rmcp_client_request_shape() -> anyhow::Result<()> {
    let (client, url, ct, lists) = spawn_counting(modern_required_config()).await;

    let body = r#"{"jsonrpc":"2.0","id":7,"method":"tools/list","params":{}}"#;
    let response = post_seam(&client, &url, body, Some("2025-11-25"), &[]).await;
    assert_eq!(response.status(), 400);
    let payload: serde_json::Value = response.json().await?;
    assert_eq!(payload["error"]["code"], -32602);
    assert_eq!(
        lists.load(Ordering::SeqCst),
        0,
        "an older client request without per-request metadata must not dispatch"
    );

    ct.cancel();
    Ok(())
}

// When the 2026+ header is present but `Mcp-Method` is missing, the
// existing `validate_standard_headers` rule must fire first with -32020,
// before the seam's -32602 missing-meta check.
#[tokio::test]
async fn seam_opt_in_preserves_standard_header_precedence() -> anyhow::Result<()> {
    let (client, url, ct, lists) = spawn_counting(modern_required_config()).await;

    let body = r#"{"jsonrpc":"2.0","id":7,"method":"tools/list","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28"}}}"#;
    let response = post_seam(&client, &url, body, Some("2026-07-28"), &[]).await;
    assert_eq!(response.status(), 400);
    let payload: serde_json::Value = response.json().await?;
    assert_eq!(
        payload["error"]["code"], -32020,
        "standard-headers routing-header error must precede the seam's meta check"
    );
    assert_eq!(
        lists.load(Ordering::SeqCst),
        0,
        "handler invocation counter must stay at 0 when a routing header is missing"
    );

    ct.cancel();
    Ok(())
}

// `initialize` is exempt from the new required-header check while retaining
// its existing optional-header and header/body consistency rules.
#[tokio::test]
async fn seam_opt_in_preserves_initialize_rules() -> anyhow::Result<()> {
    let (client, url, ct) = spawn_server(modern_required_config()).await;

    let response = post_init(&client, &url, None, "2025-11-25").await;
    assert_eq!(response.status(), 200);

    let response = post_init(&client, &url, Some("2025-11-25"), "2025-11-25").await;
    assert_eq!(response.status(), 200);

    let response = post_init(&client, &url, Some("2025-03-26"), "2025-11-25").await;
    assert_eq!(response.status(), 400);
    let payload: serde_json::Value = response.json().await?;
    assert_eq!(payload["error"]["code"], -32600);

    ct.cancel();
    Ok(())
}

// `notifications/initialized` returns HTTP 202 (Accepted) without
// requiring any protocol metadata, even under the seam.
#[tokio::test]
async fn seam_opt_in_notifications_return_202() -> anyhow::Result<()> {
    let (client, url, ct) = spawn_server(modern_required_config()).await;

    let body = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
    let response = post_seam(&client, &url, body, None, &[]).await;
    assert_eq!(
        response.status(),
        reqwest::StatusCode::ACCEPTED,
        "notifications must surface HTTP 202 regardless of metadata"
    );

    ct.cancel();
    Ok(())
}

// `server/discover` is still subject to the new HTTP-header precheck.
#[tokio::test]
async fn seam_opt_in_discover_rejects_missing_header() -> anyhow::Result<()> {
    let (client, url, ct) = spawn_server(modern_required_config()).await;

    let with_meta = r#"{"jsonrpc":"2.0","id":8,"method":"server/discover","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2025-11-25","io.modelcontextprotocol/clientInfo":{"name":"test","version":"1.0"},"io.modelcontextprotocol/clientCapabilities":{}}}}"#;
    let without_meta = r#"{"jsonrpc":"2.0","id":9,"method":"server/discover","params":{}}"#;
    for body in [with_meta, without_meta] {
        let response = post_seam(
            &client,
            &url,
            body,
            None,
            &[("Mcp-Method", "server/discover")],
        )
        .await;
        assert_eq!(response.status(), 400);
        let payload: serde_json::Value = response.json().await?;
        assert_eq!(payload["error"]["code"], -32020);
    }

    let response = post_seam(
        &client,
        &url,
        without_meta,
        Some("2025-11-25"),
        &[("Mcp-Method", "server/discover")],
    )
    .await;
    assert_eq!(response.status(), 400);
    let payload: serde_json::Value = response.json().await?;
    assert_eq!(payload["error"]["code"], -32602);

    let response = post_seam(
        &client,
        &url,
        with_meta,
        Some("2025-11-25"),
        &[("Mcp-Method", "server/discover")],
    )
    .await;
    assert_eq!(response.status(), 200);

    ct.cancel();
    Ok(())
}
