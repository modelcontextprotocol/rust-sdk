//! Tests for protocol version negotiation in stateless HTTP mode.
//!
//! Supported versions are echoed back; unknown versions, and versions outside
//! the server's `supported_protocol_versions`, fall back to the handler's own
//! version.
#![cfg(not(feature = "local"))]

use std::borrow::Cow;

use rmcp::{
    ErrorData, RoleServer, ServerHandler,
    model::{InitializeRequestParams, InitializeResult, ProtocolVersion, ServerCapabilities},
    service::RequestContext,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Default)]
struct OverridingInitialize;

impl ServerHandler for OverridingInitialize {
    fn get_info(&self) -> InitializeResult {
        InitializeResult::new(ServerCapabilities::default())
    }

    async fn initialize(
        &self,
        _request: InitializeRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, ErrorData> {
        Ok(self.get_info())
    }
}

/// Every known version except `2026-07-28`, standing in for a server that has
/// not implemented that revision.
const NARROWED_VERSIONS: &[ProtocolVersion] = &[
    ProtocolVersion::V_2024_11_05,
    ProtocolVersion::V_2025_03_26,
    ProtocolVersion::V_2025_06_18,
    ProtocolVersion::V_2025_11_25,
];

/// Overrides `initialize`, so the handler-side default negotiation never runs,
/// *and* narrows the supported versions.
#[derive(Clone, Default)]
struct NarrowedOverridingInitialize;

impl ServerHandler for NarrowedOverridingInitialize {
    fn get_info(&self) -> InitializeResult {
        InitializeResult::new(ServerCapabilities::default())
    }

    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Borrowed(NARROWED_VERSIONS)
    }

    async fn initialize(
        &self,
        _request: InitializeRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, ErrorData> {
        Ok(self.get_info())
    }
}

fn stateless_sse_config() -> StreamableHttpServerConfig {
    StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_sse_keep_alive(None)
        .with_cancellation_token(CancellationToken::new())
}

fn stateless_json_config() -> StreamableHttpServerConfig {
    stateless_sse_config().with_json_response(true)
}

async fn spawn_server(
    config: StreamableHttpServerConfig,
) -> (reqwest::Client, String, CancellationToken) {
    spawn_server_of::<OverridingInitialize>(config).await
}

async fn spawn_server_of<H: ServerHandler + Default>(
    config: StreamableHttpServerConfig,
) -> (reqwest::Client, String, CancellationToken) {
    let ct = config.cancellation_token.clone();
    let service: StreamableHttpService<H, LocalSessionManager> =
        StreamableHttpService::new(|| Ok(H::default()), Default::default(), config);

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

async fn post_init(client: &reqwest::Client, url: &str, body_version: &str) -> serde_json::Value {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": body_version,
            "capabilities": {},
            "clientInfo": {"name": "test", "version": "0.0.1"}
        }
    });
    let resp = client
        .post(url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .body(body.to_string())
        .send()
        .await
        .expect("send request");
    assert!(resp.status().is_success(), "HTTP {}", resp.status());
    let is_json = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("application/json"));
    if is_json {
        resp.json().await.expect("parse JSON")
    } else {
        let body = resp.text().await.expect("read SSE body");
        let data = body
            .lines()
            .find_map(|line| line.strip_prefix("data:"))
            .map(str::trim)
            .filter(|data| !data.is_empty())
            .expect("SSE response contains data");
        serde_json::from_str(data).expect("parse SSE data")
    }
}

#[tokio::test]
async fn stateless_json_init_echoes_known_versions_when_handler_overrides_initialize() {
    let (client, url, ct) = spawn_server(stateless_json_config()).await;

    for version in ProtocolVersion::KNOWN_VERSIONS {
        let resp = post_init(&client, &url, version.as_str()).await;
        assert_eq!(
            resp["result"]["protocolVersion"],
            version.as_str(),
            "known version {version} should be echoed back"
        );
    }

    ct.cancel();
}

#[tokio::test]
async fn stateless_sse_init_echoes_known_versions_when_handler_overrides_initialize() {
    let (client, url, ct) = spawn_server(stateless_sse_config()).await;

    for version in ProtocolVersion::KNOWN_VERSIONS {
        let resp = post_init(&client, &url, version.as_str()).await;
        assert_eq!(
            resp["result"]["protocolVersion"],
            version.as_str(),
            "known version {version} should be echoed back"
        );
    }

    ct.cancel();
}

#[tokio::test]
async fn stateless_json_init_preserves_handler_fallback_for_unknown_version() {
    let (client, url, ct) = spawn_server(stateless_json_config()).await;

    let resp = post_init(&client, &url, "1999-01-01").await;
    assert_eq!(
        resp["result"]["protocolVersion"],
        ProtocolVersion::LATEST.as_str(),
        "unknown version should preserve the handler's fallback"
    );

    ct.cancel();
}

#[tokio::test]
async fn stateless_json_init_echoes_versions_the_server_narrowed_to() {
    let (client, url, ct) =
        spawn_server_of::<NarrowedOverridingInitialize>(stateless_json_config()).await;

    for version in NARROWED_VERSIONS {
        let resp = post_init(&client, &url, version.as_str()).await;
        assert_eq!(
            resp["result"]["protocolVersion"],
            version.as_str(),
            "supported version {version} should be echoed back"
        );
    }

    ct.cancel();
}

#[tokio::test]
async fn stateless_json_init_does_not_agree_to_version_outside_supported_list() {
    let (client, url, ct) =
        spawn_server_of::<NarrowedOverridingInitialize>(stateless_json_config()).await;

    let resp = post_init(&client, &url, ProtocolVersion::V_2026_07_28.as_str()).await;
    assert_eq!(
        resp["result"]["protocolVersion"],
        ProtocolVersion::V_2025_11_25.as_str(),
        "a version outside supported_protocol_versions should not be echoed back"
    );

    ct.cancel();
}
