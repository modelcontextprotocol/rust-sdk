#![cfg(all(
    not(feature = "local"),
    feature = "reqwest",
    feature = "transport-streamable-http-server"
))]

use std::borrow::Cow;

use rmcp::{
    ServerHandler,
    model::{Implementation, ProtocolVersion, ServerCapabilities, ServerInfo},
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
};
use serde_json::json;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Default)]
struct DiscoveryServer;

impl ServerHandler for DiscoveryServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("discovery-server", "1.0.0"))
            .with_instructions("Use the tools carefully")
    }

    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Borrowed(&[ProtocolVersion::V_2025_11_25])
    }
}

async fn spawn_server(json_response: bool) -> (reqwest::Client, String, CancellationToken) {
    spawn_server_with_legacy_session_mode(json_response, false).await
}

async fn spawn_server_with_legacy_session_mode(
    json_response: bool,
    legacy_session_mode: bool,
) -> (reqwest::Client, String, CancellationToken) {
    let cancellation_token = CancellationToken::new();
    let config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(legacy_session_mode)
        .with_json_response(json_response)
        .with_sse_keep_alive(None)
        .with_cancellation_token(cancellation_token.clone());
    let service: StreamableHttpService<DiscoveryServer, LocalSessionManager> =
        StreamableHttpService::new(|| Ok(DiscoveryServer), Default::default(), config);
    let router = axum::Router::new().nest_service("/mcp", service);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener should bind");
    let address = listener.local_addr().expect("listener should have address");

    tokio::spawn({
        let cancellation_token = cancellation_token.clone();
        async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    cancellation_token.cancelled_owned().await;
                })
                .await;
        }
    });

    (
        reqwest::Client::new(),
        format!("http://{address}/mcp"),
        cancellation_token,
    )
}

fn discover_body(version: Option<&str>) -> serde_json::Value {
    let meta = version.map(|version| {
        json!({
            "io.modelcontextprotocol/protocolVersion": version,
            "io.modelcontextprotocol/clientInfo": {
                "name": "test-client",
                "version": "1.0.0"
            },
            "io.modelcontextprotocol/clientCapabilities": {}
        })
    });
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "server/discover",
        "params": meta.map(|meta| json!({ "_meta": meta })).unwrap_or_else(|| json!({}))
    })
}

async fn post_discover(
    client: &reqwest::Client,
    url: &str,
    header_version: &str,
    body_version: Option<&str>,
) -> reqwest::Response {
    client
        .post(url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", header_version)
        .header("Mcp-Method", "server/discover")
        .json(&discover_body(body_version))
        .send()
        .await
        .expect("discover request should send")
}

#[tokio::test]
async fn discover_returns_server_metadata_without_session() {
    let (client, url, cancellation_token) = spawn_server(true).await;

    let response = post_discover(&client, &url, "2025-11-25", Some("2025-11-25")).await;

    assert_eq!(response.status(), 200);
    let body: serde_json::Value = response.json().await.expect("response should be JSON");
    assert_eq!(
        body["result"],
        json!({
            "resultType": "complete",
            "supportedVersions": ["2025-11-25"],
            "capabilities": { "tools": {} },
            "serverInfo": {
                "name": "discovery-server",
                "version": "1.0.0"
            },
            "instructions": "Use the tools carefully",
            "ttlMs": 0,
            "cacheScope": "private"
        })
    );

    cancellation_token.cancel();
}

#[tokio::test]
async fn discover_does_not_require_initialization_in_legacy_session_mode() {
    let (client, url, cancellation_token) = spawn_server_with_legacy_session_mode(true, true).await;

    let response = post_discover(&client, &url, "2025-11-25", Some("2025-11-25")).await;

    assert_eq!(response.status(), 200);
    assert!(response.headers().get("Mcp-Session-Id").is_none());

    cancellation_token.cancel();
}

#[tokio::test]
async fn discover_rejects_unsupported_version_with_http_400() {
    let (client, url, cancellation_token) = spawn_server(true).await;

    let response = post_discover(&client, &url, "2026-07-28", Some("2026-07-28")).await;

    assert_eq!(response.status(), 400);
    let body: serde_json::Value = response.json().await.expect("response should be JSON");
    assert_eq!(body["error"]["code"], -32022);
    assert_eq!(
        body["error"]["data"],
        json!({
            "requested": "2026-07-28",
            "supported": ["2025-11-25"]
        })
    );

    cancellation_token.cancel();
}

#[tokio::test]
async fn discover_rejects_unknown_version_with_typed_error() {
    let (client, url, cancellation_token) = spawn_server(true).await;

    let response = post_discover(&client, &url, "2099-01-01", Some("2099-01-01")).await;

    assert_eq!(response.status(), 400);
    let body: serde_json::Value = response.json().await.expect("response should be JSON");
    assert_eq!(body["error"]["code"], -32022);

    cancellation_token.cancel();
}

#[tokio::test]
async fn regular_request_rejects_server_unsupported_meta_version() {
    let (client, url, cancellation_token) = spawn_server(true).await;
    let body = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/list",
        "params": {
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": "2026-07-28"
            }
        }
    });

    let response = client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "tools/list")
        .json(&body)
        .send()
        .await
        .expect("request should send");

    assert_eq!(response.status(), 400);
    let body: serde_json::Value = response.json().await.expect("response should be JSON");
    assert_eq!(body["error"]["code"], -32022);

    cancellation_token.cancel();
}

#[tokio::test]
async fn unknown_rpc_uses_http_404_for_per_request_protocol() {
    let (client, url, cancellation_token) = spawn_server(true).await;
    let body = json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "unknown/method",
        "params": {
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": "2025-11-25"
            }
        }
    });

    let response = client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2025-11-25")
        .json(&body)
        .send()
        .await
        .expect("request should send");

    assert_eq!(response.status(), 404);

    cancellation_token.cancel();
}

#[tokio::test]
async fn legacy_unknown_rpc_preserves_http_200_jsonrpc_error() {
    let (client, url, cancellation_token) = spawn_server(true).await;
    let body = json!({
        "jsonrpc": "2.0",
        "id": 5,
        "method": "unknown/method",
        "params": {}
    });

    let response = client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2025-11-25")
        .json(&body)
        .send()
        .await
        .expect("request should send");

    assert_eq!(response.status(), 200);
    let body: serde_json::Value = response.json().await.expect("response should be JSON");
    assert_eq!(body["error"]["code"], -32601);

    cancellation_token.cancel();
}

#[tokio::test]
async fn discover_rejects_header_meta_version_mismatch() {
    let (client, url, cancellation_token) = spawn_server(true).await;

    let response = post_discover(&client, &url, "2026-07-28", Some("2025-11-25")).await;

    assert_eq!(response.status(), 400);

    cancellation_token.cancel();
}

#[tokio::test]
async fn discover_rejects_missing_request_meta() {
    let (client, url, cancellation_token) = spawn_server(true).await;

    let response = post_discover(&client, &url, "2025-11-25", None).await;

    assert_eq!(response.status(), 400);
    let body: serde_json::Value = response.json().await.expect("response should be JSON");
    assert_eq!(body["error"]["code"], -32602);

    cancellation_token.cancel();
}

#[tokio::test]
async fn discover_rejects_missing_client_capabilities() {
    let (client, url, cancellation_token) = spawn_server(true).await;
    let body = json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "server/discover",
        "params": {
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": "2025-11-25",
                "io.modelcontextprotocol/clientInfo": {
                    "name": "test-client",
                    "version": "1.0.0"
                }
            }
        }
    });

    let response = client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2025-11-25")
        .json(&body)
        .send()
        .await
        .expect("request should send");

    assert_eq!(response.status(), 400);
    let body: serde_json::Value = response.json().await.expect("response should be JSON");
    assert_eq!(body["error"]["code"], -32602);

    cancellation_token.cancel();
}

#[tokio::test]
async fn discover_error_uses_http_400_when_sse_is_configured() {
    let (client, url, cancellation_token) = spawn_server(false).await;

    let response = post_discover(&client, &url, "2026-07-28", Some("2026-07-28")).await;

    assert_eq!(response.status(), 400);
    assert_eq!(
        response
            .headers()
            .get("Content-Type")
            .and_then(|value| value.to_str().ok()),
        Some("application/json")
    );

    cancellation_token.cancel();
}
