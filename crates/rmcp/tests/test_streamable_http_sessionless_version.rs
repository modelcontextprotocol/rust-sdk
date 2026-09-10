#![cfg(all(
    not(feature = "local"),
    feature = "client",
    feature = "reqwest",
    feature = "transport-streamable-http-server"
))]
//! SEP-2567 removes sessions and the standalone GET stream at 2026-07-28. A legacy-shaped
//! handshake can still answer with an `Mcp-Session-Id` while negotiating that version; the
//! client must not then echo the id or open the stream.

use std::sync::{Arc, Mutex};

use axum::{
    Router,
    body::{Body, Bytes},
    extract::State,
    http::{HeaderMap, Response, StatusCode},
    routing::any,
};
use rmcp::{
    ClientLifecycleMode, ClientServiceExt,
    model::ClientInfo,
    transport::{
        StreamableHttpClientTransport, streamable_http_client::StreamableHttpClientTransportConfig,
    },
};
use serde_json::json;
use tokio_util::sync::CancellationToken;

const SESSION_ID: &str = "session-the-server-should-not-have-issued";

/// One request the client made: HTTP method, JSON-RPC method, `Mcp-Session-Id` header.
type Call = (String, String, Option<String>);

#[derive(Clone, Default)]
struct Recorder {
    seen: Arc<Mutex<Vec<Call>>>,
    negotiated_version: Arc<Mutex<String>>,
}

impl Recorder {
    fn calls(&self) -> Vec<Call> {
        self.seen.lock().expect("recorder poisoned").clone()
    }

    fn get_requests(&self) -> Vec<Call> {
        self.calls()
            .into_iter()
            .filter(|(http_method, ..)| http_method == "GET")
            .collect()
    }

    fn session_headers_after_handshake(&self) -> Vec<Option<String>> {
        self.calls()
            .into_iter()
            .filter(|(_, jsonrpc_method, _)| jsonrpc_method != "initialize")
            .map(|(.., session)| session)
            .collect()
    }
}

async fn handler(
    State(state): State<Recorder>,
    method: axum::http::Method,
    headers: HeaderMap,
    body: Bytes,
) -> Response<Body> {
    let session = headers
        .get("mcp-session-id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);

    if method == axum::http::Method::GET {
        state.seen.lock().expect("recorder poisoned").push((
            "GET".to_owned(),
            "-".to_owned(),
            session,
        ));
        // Hang so the client keeps the stream if it opens one; the test cancels it.
        return Response::builder()
            .status(StatusCode::METHOD_NOT_ALLOWED)
            .body(Body::empty())
            .expect("build GET rejection");
    }

    let request: serde_json::Value = serde_json::from_slice(&body).expect("valid JSON-RPC body");
    let jsonrpc_method = request["method"].as_str().unwrap_or("-").to_owned();
    state.seen.lock().expect("recorder poisoned").push((
        "POST".to_owned(),
        jsonrpc_method.clone(),
        session,
    ));

    if jsonrpc_method == "server/discover" {
        // Force the legacy initialize path.
        return Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::empty())
            .expect("build discover rejection");
    }

    if jsonrpc_method == "initialize" {
        let version = state
            .negotiated_version
            .lock()
            .expect("recorder poisoned")
            .clone();
        // The shape this issue is about: a session id alongside a negotiated version
        // that, from 2026-07-28 on, has no sessions at all.
        return Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "application/json")
            .header("mcp-session-id", SESSION_ID)
            .body(Body::from(
                json!({
                    "jsonrpc": "2.0",
                    "id": request["id"],
                    "result": {
                        "protocolVersion": version,
                        "capabilities": {},
                        "serverInfo": {"name": "dual-era", "version": "1.0"}
                    }
                })
                .to_string(),
            ))
            .expect("build initialize response");
    }

    Response::builder()
        .status(StatusCode::ACCEPTED)
        .body(Body::empty())
        .expect("build notification response")
}

async fn connect_and_record(negotiated_version: &str) -> Recorder {
    let recorder = Recorder::default();
    *recorder
        .negotiated_version
        .lock()
        .expect("recorder poisoned") = negotiated_version.to_owned();

    let ct = CancellationToken::new();
    let router = Router::new()
        .route("/mcp", any(handler))
        .with_state(recorder.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener should bind");
    let address = listener.local_addr().expect("listener address");
    let server = tokio::spawn({
        let ct = ct.clone();
        async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async move { ct.cancelled_owned().await })
                .await;
        }
    });

    let transport = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(format!("http://{address}/mcp")),
    );
    let client = ClientInfo::default()
        .serve_with_lifecycle(transport, ClientLifecycleMode::Initialize)
        .await
        .expect("client should start against a legacy handshake");

    // Give a standalone GET stream, if one were opened, time to reach the server.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    client.cancel().await.expect("cancel client");
    ct.cancel();
    let _ = server.await;
    recorder
}

#[tokio::test]
async fn modern_version_drops_the_session_and_opens_no_stream() {
    let recorder = connect_and_record("2026-07-28").await;

    // No standalone GET stream: SEP-2567 removed the endpoint at this version.
    assert_eq!(
        recorder.get_requests(),
        Vec::new(),
        "client opened a standalone GET stream at a version that has none"
    );
    // And the id the server volunteered is not echoed back on anything.
    let sessions = recorder.session_headers_after_handshake();
    assert!(
        !sessions.is_empty(),
        "expected at least one post-handshake request to inspect"
    );
    assert!(
        sessions.iter().all(Option::is_none),
        "client echoed Mcp-Session-Id at a version with no sessions: {sessions:?}"
    );
}

#[tokio::test]
async fn legacy_version_keeps_the_session_and_opens_the_stream() {
    let recorder = connect_and_record("2025-11-25").await;

    // The legacy shape is untouched: the stream is opened and carries the session id.
    let gets = recorder.get_requests();
    assert_eq!(
        gets.len(),
        1,
        "expected exactly one standalone GET stream on a legacy session, got {gets:?}"
    );
    assert_eq!(gets[0].2.as_deref(), Some(SESSION_ID));
    let sessions = recorder.session_headers_after_handshake();
    assert!(
        sessions.iter().any(|s| s.as_deref() == Some(SESSION_ID)),
        "legacy session id was not echoed after the handshake: {sessions:?}"
    );
}
