#![cfg(all(
    not(feature = "local"),
    feature = "client",
    feature = "reqwest",
    feature = "transport-streamable-http-server"
))]
//! SEP-2567 removes sessions and the standalone GET stream at 2026-07-28. A legacy-shaped
//! handshake can still answer with an `Mcp-Session-Id` while negotiating that version; the
//! client must not then echo the id or open the stream. That holds for the replacement
//! handshake after an expired-session 404 as much as for the first one.

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

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
const REPLACEMENT_SESSION_ID: &str = "session-issued-by-the-replacement-handshake";

/// One request the client made: HTTP method, JSON-RPC method, `Mcp-Session-Id` header.
type Call = (String, String, Option<String>);

/// How the server answers one `initialize`: the id it volunteers and the version it settles on.
type Handshake = (&'static str, &'static str);

#[derive(Clone)]
struct Recorder {
    seen: Arc<Mutex<Vec<Call>>>,
    /// Consumed one per `initialize`, in order; the last entry answers every later one.
    handshakes: Arc<Mutex<VecDeque<Handshake>>>,
    /// While set, the next `tools/list` is refused with a 404 to expire the session.
    expire_next_list: Arc<Mutex<bool>>,
}

impl Recorder {
    /// A server that answers every handshake the same way and never expires a session.
    fn new(handshake: Handshake) -> Self {
        Self {
            seen: Arc::new(Mutex::new(Vec::new())),
            handshakes: Arc::new(Mutex::new(VecDeque::from([handshake]))),
            expire_next_list: Arc::new(Mutex::new(false)),
        }
    }

    /// A server that expires the first session, then answers the replacement handshake
    /// differently from the first.
    fn expiring(first: Handshake, replacement: Handshake) -> Self {
        Self {
            seen: Arc::new(Mutex::new(Vec::new())),
            handshakes: Arc::new(Mutex::new(VecDeque::from([first, replacement]))),
            expire_next_list: Arc::new(Mutex::new(true)),
        }
    }

    fn next_handshake(&self) -> Handshake {
        let mut handshakes = self.handshakes.lock().expect("recorder poisoned");
        if handshakes.len() > 1 {
            handshakes.pop_front().expect("checked non-empty")
        } else {
            *handshakes
                .front()
                .expect("at least one handshake is scripted")
        }
    }

    fn take_expiry(&self) -> bool {
        std::mem::replace(
            &mut self.expire_next_list.lock().expect("recorder poisoned"),
            false,
        )
    }

    fn calls(&self) -> Vec<Call> {
        self.seen.lock().expect("recorder poisoned").clone()
    }

    fn get_requests(&self) -> Vec<Call> {
        self.calls()
            .into_iter()
            .filter(|(http_method, ..)| http_method == "GET")
            .collect()
    }

    fn delete_requests(&self) -> Vec<Call> {
        self.calls()
            .into_iter()
            .filter(|(http_method, ..)| http_method == "DELETE")
            .collect()
    }

    /// Session headers on everything after the handshake that is not the teardown DELETE,
    /// which carries the id on purpose so the server tears the session down.
    fn session_headers_after_handshake(&self) -> Vec<Option<String>> {
        self.calls()
            .into_iter()
            .filter(|(http_method, jsonrpc_method, _)| {
                jsonrpc_method != "initialize" && http_method != "DELETE"
            })
            .map(|(.., session)| session)
            .collect()
    }

    /// Everything the client sent after the last `initialize`, i.e. on the session that
    /// handshake established. Empty if it never re-initialized.
    fn calls_on_replacement_session(&self) -> Vec<Call> {
        let calls = self.calls();
        match calls
            .iter()
            .rposition(|(_, jsonrpc_method, _)| jsonrpc_method == "initialize")
        {
            Some(last_initialize) => calls[last_initialize + 1..].to_vec(),
            None => Vec::new(),
        }
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

    // The shutdown DELETE carries no body, so record it before anything parses one.
    if method == axum::http::Method::DELETE {
        state.seen.lock().expect("recorder poisoned").push((
            "DELETE".to_owned(),
            "-".to_owned(),
            session,
        ));
        return Response::builder()
            .status(StatusCode::OK)
            .body(Body::empty())
            .expect("build session teardown response");
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
        let (session_id, version) = state.next_handshake();
        // The shape this issue is about: a session id alongside a negotiated version
        // that, from 2026-07-28 on, has no sessions at all.
        return Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "application/json")
            .header("mcp-session-id", session_id)
            .body(Body::from(
                json!({
                    "jsonrpc": "2.0",
                    "id": request["id"],
                    "result": {
                        "protocolVersion": version,
                        "capabilities": {"tools": {}},
                        "serverInfo": {"name": "dual-era", "version": "1.0"}
                    }
                })
                .to_string(),
            ))
            .expect("build initialize response");
    }

    if jsonrpc_method == "tools/list" {
        if state.take_expiry() {
            // The expired-session 404 that sends the client through re-initialization.
            return Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Body::empty())
                .expect("build session-expired rejection");
        }
        return Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "application/json")
            .body(Body::from(
                json!({
                    "jsonrpc": "2.0",
                    "id": request["id"],
                    "result": {"tools": []}
                })
                .to_string(),
            ))
            .expect("build tools/list response");
    }

    Response::builder()
        .status(StatusCode::ACCEPTED)
        .body(Body::empty())
        .expect("build notification response")
}

/// Serve `recorder` on a loopback port until the returned token is cancelled.
async fn serve(recorder: &Recorder) -> (String, CancellationToken, tokio::task::JoinHandle<()>) {
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
    (format!("http://{address}/mcp"), ct, server)
}

async fn start_client(uri: String) -> rmcp::service::RunningService<rmcp::RoleClient, ClientInfo> {
    let transport = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(uri),
    );
    ClientInfo::default()
        .serve_with_lifecycle(transport, ClientLifecycleMode::Initialize)
        .await
        .expect("client should start against a legacy handshake")
}

async fn connect_and_record(negotiated_version: &'static str) -> Recorder {
    let recorder = Recorder::new((SESSION_ID, negotiated_version));
    let (uri, ct, server) = serve(&recorder).await;
    let client = start_client(uri).await;

    // Give a standalone GET stream, if one were opened, time to reach the server.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    client.cancel().await.expect("cancel client");
    ct.cancel();
    let _ = server.await;
    recorder
}

/// Start on a legacy session, let the server expire it, and come back on a replacement
/// handshake that negotiates `replacement_version`.
async fn connect_through_recovery(replacement_version: &'static str) -> Recorder {
    let recorder = Recorder::expiring(
        (SESSION_ID, "2025-11-25"),
        (REPLACEMENT_SESSION_ID, replacement_version),
    );
    let (uri, ct, server) = serve(&recorder).await;
    let client = start_client(uri).await;

    // The first tools/list is answered 404; the transport re-initializes and retries it.
    client
        .peer()
        .list_tools(None)
        .await
        .expect("the retry after re-initialization should succeed");

    // Give a standalone GET stream on the replacement session, if one were opened, time
    // to reach the server.
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

#[tokio::test]
async fn replacement_handshake_at_a_modern_version_drops_the_session_too() {
    let recorder = connect_through_recovery("2026-07-28").await;

    let replacement = recorder.calls_on_replacement_session();
    assert!(
        !replacement.is_empty(),
        "the session was never re-established: {:?}",
        recorder.calls()
    );

    // The `initialized` notification completes the replacement handshake, so it is part of
    // the new session and must not carry an id that version has no sessions for.
    let initialized = replacement
        .iter()
        .find(|(_, jsonrpc_method, _)| jsonrpc_method == "notifications/initialized")
        .expect("replacement handshake should send notifications/initialized");
    assert_eq!(
        initialized.2, None,
        "re-initialization echoed Mcp-Session-Id on the notification that completes it"
    );

    // Nor on anything after it, and no stream on the replacement session either.
    let echoed: Vec<_> = replacement
        .iter()
        .filter(|(http_method, _, session)| session.is_some() && http_method != "DELETE")
        .collect();
    assert!(
        echoed.is_empty(),
        "client echoed Mcp-Session-Id on the replacement session: {echoed:?}"
    );
    assert_eq!(
        recorder.get_requests().len(),
        1,
        "expected only the legacy session's GET stream, got {:?}",
        recorder.get_requests()
    );

    // Dropping the id is a request-and-stream decision, not a licence to leak server
    // state: the session the server really did create is still torn down at shutdown.
    let deletes = recorder.delete_requests();
    assert!(
        deletes
            .iter()
            .any(|(.., session)| session.as_deref() == Some(REPLACEMENT_SESSION_ID)),
        "the replacement session was never deleted at shutdown: {deletes:?}"
    );
}

#[tokio::test]
async fn replacement_handshake_at_a_legacy_version_keeps_its_session() {
    let recorder = connect_through_recovery("2025-11-25").await;

    let replacement = recorder.calls_on_replacement_session();
    assert!(
        !replacement.is_empty(),
        "the session was never re-established: {:?}",
        recorder.calls()
    );
    assert!(
        replacement
            .iter()
            .any(|(.., session)| session.as_deref() == Some(REPLACEMENT_SESSION_ID)),
        "replacement session id was not echoed on a legacy session: {replacement:?}"
    );
    assert_eq!(
        recorder.get_requests().len(),
        2,
        "expected a GET stream on each legacy session, got {:?}",
        recorder.get_requests()
    );
}
