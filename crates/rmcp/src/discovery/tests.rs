use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};

use super::*;

// ---------------------------------------------------------------------------
// Mock implementations
// ---------------------------------------------------------------------------

/// In-memory DNS resolver: maps a TXT label to its records.
#[derive(Default, Clone)]
struct MockDns {
    records: HashMap<String, Vec<String>>,
}

impl MockDns {
    fn with(mut self, name: &str, records: &[&str]) -> Self {
        self.records.insert(
            name.to_string(),
            records.iter().map(|s| s.to_string()).collect(),
        );
        self
    }
}

#[async_trait]
impl DnsResolver for MockDns {
    async fn txt_lookup(&self, name: &str) -> std::result::Result<Vec<String>, DnsLookupError> {
        Ok(self.records.get(name).cloned().unwrap_or_default())
    }
}

/// In-memory HTTP fetcher: maps URL -> body for GET, and a set of reachable
/// endpoints for probe.
#[derive(Default, Clone)]
struct MockHttp {
    bodies: HashMap<String, String>,
    reachable: Vec<String>,
    probed: Arc<Mutex<Vec<String>>>,
}

impl MockHttp {
    fn with_body(mut self, url: &str, body: &str) -> Self {
        self.bodies.insert(url.to_string(), body.to_string());
        self
    }
    fn reachable(mut self, url: &str) -> Self {
        self.reachable.push(url.to_string());
        self
    }
}

#[async_trait]
impl ManifestFetcher for MockHttp {
    async fn get(&self, url: &str) -> Result<FetchOutcome> {
        match self.bodies.get(url) {
            Some(body) => Ok(FetchOutcome::Found { body: body.clone() }),
            None => Ok(FetchOutcome::NotFound),
        }
    }
    async fn probe(&self, url: &str) -> Result<bool> {
        self.probed.lock().unwrap().push(url.to_string());
        Ok(self.reachable.iter().any(|u| u == url))
    }
}

fn default_opts() -> DiscoveryOptions {
    DiscoveryOptions::default()
}

// ---------------------------------------------------------------------------
// URI parsing tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn rejects_non_mcp_scheme() {
    let http = MockHttp::default();
    let err = resolve_with("https://example.com", None, &http, &default_opts())
        .await
        .unwrap_err();
    assert!(matches!(err, DiscoveryError::InvalidUri(_)));
}

#[tokio::test]
async fn rejects_missing_host() {
    let http = MockHttp::default();
    let err = resolve_with("mcp:example.com", None, &http, &default_opts())
        .await
        .unwrap_err();
    assert!(matches!(err, DiscoveryError::InvalidUri(_)));
}

// ---------------------------------------------------------------------------
// Well-known manifest resolution
// ---------------------------------------------------------------------------

#[tokio::test]
async fn well_known_manifest_resolves() {
    let body = r#"{"mcp_version":"2025-06-18","name":"Example","endpoint":"https://example.com/mcp","transport":"http"}"#;
    let http = MockHttp::default().with_body("https://example.com/.well-known/mcp-server", body);
    let server = resolve_with("mcp://example.com", None, &http, &default_opts())
        .await
        .unwrap();
    assert_eq!(server.source, DiscoverySource::WellKnown);
    assert_eq!(server.endpoint, "https://example.com/mcp");
    assert_eq!(server.trust_class, TrustClass::Public);
    assert!(!server.signature_verified);

    let url = server.endpoint_url().unwrap();
    assert_eq!(url.as_str(), "https://example.com/mcp");
}

#[tokio::test]
async fn subdomain_endpoint_is_allowed() {
    let body = r#"{"mcp_version":"1","name":"x","endpoint":"https://api.example.com/mcp","transport":"http"}"#;
    let http = MockHttp::default().with_body("https://example.com/.well-known/mcp-server", body);
    let server = resolve_with("mcp://example.com", None, &http, &default_opts())
        .await
        .unwrap();
    assert_eq!(server.endpoint, "https://api.example.com/mcp");
}

#[tokio::test]
async fn endpoint_host_mismatch_is_rejected() {
    let body =
        r#"{"mcp_version":"1","name":"x","endpoint":"https://evil.com/mcp","transport":"http"}"#;
    let http = MockHttp::default().with_body("https://example.com/.well-known/mcp-server", body);
    let err = resolve_with("mcp://example.com", None, &http, &default_opts())
        .await
        .unwrap_err();
    assert!(matches!(err, DiscoveryError::EndpointHostMismatch { .. }));
}

#[tokio::test]
async fn insecure_endpoint_is_rejected() {
    let body =
        r#"{"mcp_version":"1","name":"x","endpoint":"http://example.com/mcp","transport":"http"}"#;
    let http = MockHttp::default().with_body("https://example.com/.well-known/mcp-server", body);
    let err = resolve_with("mcp://example.com", None, &http, &default_opts())
        .await
        .unwrap_err();
    assert!(matches!(err, DiscoveryError::InsecureEndpoint(_)));
}

#[tokio::test]
async fn malformed_manifest_is_rejected() {
    let http =
        MockHttp::default().with_body("https://example.com/.well-known/mcp-server", "not json");
    let err = resolve_with("mcp://example.com", None, &http, &default_opts())
        .await
        .unwrap_err();
    assert!(matches!(err, DiscoveryError::MalformedManifest { .. }));
}

// ---------------------------------------------------------------------------
// Direct handshake fallback
// ---------------------------------------------------------------------------

#[tokio::test]
async fn falls_back_to_direct_handshake() {
    let http = MockHttp::default().reachable("https://example.com/mcp");
    let server = resolve_with("mcp://example.com", None, &http, &default_opts())
        .await
        .unwrap();
    assert_eq!(server.source, DiscoverySource::DirectFallback);
    assert_eq!(server.endpoint, "https://example.com/mcp");
}

#[tokio::test]
async fn fallback_preserves_uri_path() {
    let http = MockHttp::default().reachable("https://example.com/custom");
    let server = resolve_with("mcp://example.com/custom", None, &http, &default_opts())
        .await
        .unwrap();
    assert_eq!(server.source, DiscoverySource::DirectFallback);
    assert_eq!(server.endpoint, "https://example.com/custom");
}

#[tokio::test]
async fn fallback_honors_validated_dns_src() {
    // No well-known manifest, but DNS advertises a same-domain src endpoint.
    let http = MockHttp::default().reachable("https://api.example.com/mcp");
    let dns = MockDns::default().with(
        "_mcp.example.com",
        &["v=mcp1; src=https://api.example.com/mcp"],
    );
    let server = resolve_with("mcp://example.com", Some(&dns), &http, &default_opts())
        .await
        .unwrap();
    assert_eq!(server.source, DiscoverySource::DirectFallback);
    assert_eq!(server.endpoint, "https://api.example.com/mcp");
}

#[tokio::test]
async fn fallback_ignores_cross_host_dns_src() {
    // A cross-host src (a spoofed DNS answer) must be ignored; discovery falls
    // back to the default /mcp on the discovery host instead.
    let http = MockHttp::default().reachable("https://example.com/mcp");
    let dns = MockDns::default().with("_mcp.example.com", &["v=mcp1; src=https://evil.com/mcp"]);
    let server = resolve_with("mcp://example.com", Some(&dns), &http, &default_opts())
        .await
        .unwrap();
    assert_eq!(server.endpoint, "https://example.com/mcp");
}

#[tokio::test]
async fn no_server_anywhere_is_not_found() {
    let http = MockHttp::default();
    let err = resolve_with("mcp://example.com", None, &http, &default_opts())
        .await
        .unwrap_err();
    assert!(matches!(err, DiscoveryError::NotFound(_)));
}

#[tokio::test]
async fn port_is_carried_into_urls() {
    let body = r#"{"mcp_version":"1","name":"x","endpoint":"https://example.com:8080/mcp","transport":"http"}"#;
    let http =
        MockHttp::default().with_body("https://example.com:8080/.well-known/mcp-server", body);
    let server = resolve_with("mcp://example.com:8080", None, &http, &default_opts())
        .await
        .unwrap();
    assert_eq!(server.endpoint, "https://example.com:8080/mcp");
}

// ---------------------------------------------------------------------------
// Signature verification
// ---------------------------------------------------------------------------

#[tokio::test]
async fn signed_manifest_without_published_key_is_rejected() {
    let signed = r#"{"mcp_version":"1","name":"x","endpoint":"https://example.com/mcp","transport":"http","signature":{"alg":"ES256","kid":"unknown","value":"AAAA"}}"#;
    let http = MockHttp::default().with_body("https://example.com/.well-known/mcp-server", signed);
    // DNS has no key record.
    let dns = MockDns::default();
    let err = resolve_with("mcp://example.com", Some(&dns), &http, &default_opts())
        .await
        .unwrap_err();
    assert!(matches!(err, DiscoveryError::SignatureVerification(_)));
}

#[tokio::test]
async fn require_signature_rejects_unsigned_manifest() {
    let body =
        r#"{"mcp_version":"1","name":"x","endpoint":"https://example.com/mcp","transport":"http"}"#;
    let http = MockHttp::default().with_body("https://example.com/.well-known/mcp-server", body);
    let opts = DiscoveryOptions {
        require_signature: true,
        ..DiscoveryOptions::default()
    };
    let err = resolve_with("mcp://example.com", None, &http, &opts)
        .await
        .unwrap_err();
    assert!(matches!(err, DiscoveryError::SignatureVerification(_)));
}

#[tokio::test]
async fn dns_conflict_is_flagged_but_manifest_wins() {
    let body =
        r#"{"mcp_version":"1","name":"x","endpoint":"https://example.com/mcp","transport":"http"}"#;
    let http = MockHttp::default().with_body("https://example.com/.well-known/mcp-server", body);
    let dns = MockDns::default().with(
        "_mcp.example.com",
        &["v=mcp1; src=https://example.com/other"],
    );
    let server = resolve_with("mcp://example.com", Some(&dns), &http, &default_opts())
        .await
        .unwrap();
    assert_eq!(server.endpoint, "https://example.com/mcp");
    assert!(server.dns_conflict);
}

// ---------------------------------------------------------------------------
// End-to-end test with a live axum server (rmcp test convention)
// ---------------------------------------------------------------------------

/// Test server state: what manifest body to serve, and whether to act as a
/// reachable MCP endpoint.
#[derive(Clone)]
struct TestServerState {
    manifest_body: Option<String>,
    mcp_reachable: bool,
}

async fn well_known_handler(State(state): State<TestServerState>) -> Response {
    match &state.manifest_body {
        Some(body) => (
            StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            body.clone(),
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn mcp_probe_handler(State(state): State<TestServerState>) -> Response {
    if state.mcp_reachable {
        // Return a JSON-RPC response so the probe detects an MCP server.
        (
            StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            r#"{"jsonrpc":"2.0","id":1,"result":{}}"#,
        )
            .into_response()
    } else {
        StatusCode::NOT_FOUND.into_response()
    }
}

async fn start_test_server(state: TestServerState) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let app = Router::new()
        .route("/.well-known/mcp-server", get(well_known_handler))
        .route("/mcp", post(mcp_probe_handler))
        .with_state(state);

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    format!("http://{addr}")
}

#[tokio::test]
#[ignore = "requires real HTTPS server with TLS certificates"]
async fn e2e_well_known_manifest_resolution() {
    let port = pick_ephemeral_port().await;
    let base = format!("http://127.0.0.1:{port}");
    let manifest = format!(
        r#"{{"mcp_version":"1","name":"TestServer","endpoint":"{base}/mcp","transport":"http"}}"#
    );

    let state = TestServerState {
        manifest_body: Some(manifest),
        mcp_reachable: true,
    };
    let _ = start_test_server_on_port(port, state).await;

    let discovery_uri = format!("mcp://127.0.0.1:{port}");
    let server = McpDiscovery::resolve_with_options(
        &discovery_uri,
        DiscoveryOptions {
            use_dns: false,
            ..DiscoveryOptions::default()
        },
    )
    .await
    .unwrap();

    assert_eq!(server.source, DiscoverySource::WellKnown);
    assert_eq!(server.endpoint, format!("{base}/mcp"));
    assert_eq!(server.manifest.name, "TestServer");
}

#[tokio::test]
#[ignore = "requires real HTTPS server with TLS certificates"]
async fn e2e_direct_fallback_resolution() {
    let port = pick_ephemeral_port().await;
    let base = format!("http://127.0.0.1:{port}");

    let state = TestServerState {
        manifest_body: None, // no well-known → fallback
        mcp_reachable: true,
    };
    let _ = start_test_server_on_port(port, state).await;

    let discovery_uri = format!("mcp://127.0.0.1:{port}");
    let server = McpDiscovery::resolve_with_options(
        &discovery_uri,
        DiscoveryOptions {
            use_dns: false,
            ..DiscoveryOptions::default()
        },
    )
    .await
    .unwrap();

    assert_eq!(server.source, DiscoverySource::DirectFallback);
    assert_eq!(server.endpoint, format!("{base}/mcp"));
}

#[tokio::test]
#[ignore = "requires real HTTPS server with TLS certificates"]
async fn e2e_no_server_found() {
    let port = pick_ephemeral_port().await;

    let state = TestServerState {
        manifest_body: None,
        mcp_reachable: false,
    };
    let _ = start_test_server_on_port(port, state).await;

    let discovery_uri = format!("mcp://127.0.0.1:{port}");
    let err = McpDiscovery::resolve_with_options(
        &discovery_uri,
        DiscoveryOptions {
            use_dns: false,
            ..DiscoveryOptions::default()
        },
    )
    .await
    .unwrap_err();

    assert!(matches!(err, DiscoveryError::NotFound(_)));
}

// ---------------------------------------------------------------------------
// E2E helpers
// ---------------------------------------------------------------------------

async fn pick_ephemeral_port() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

async fn start_test_server_on_port(port: u16, state: TestServerState) -> SocketAddr {
    let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();

    let app = Router::new()
        .route("/.well-known/mcp-server", get(well_known_handler))
        .route("/mcp", post(mcp_probe_handler))
        .with_state(state);

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    addr
}
