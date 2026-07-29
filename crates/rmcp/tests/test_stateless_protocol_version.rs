//! Tests for protocol version negotiation in stateless HTTP mode.
//!
//! Known versions are echoed back; unknown versions fall back to LATEST.
#![cfg(not(feature = "local"))]

use rmcp::{
    ErrorData, RoleServer, ServerHandler,
    model::{
        InitializeRequestParams, InitializeResult, ProtocolVersion, ServerCapabilities, ServerInfo,
    },
    service::RequestContext,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
struct OverridingInitialize;

impl ServerHandler for OverridingInitialize {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::default())
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
    let ct = config.cancellation_token.clone();
    let service: StreamableHttpService<OverridingInitialize, LocalSessionManager> =
        StreamableHttpService::new(|| Ok(OverridingInitialize), Default::default(), config);

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
