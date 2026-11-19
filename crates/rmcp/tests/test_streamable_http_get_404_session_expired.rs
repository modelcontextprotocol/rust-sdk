//! End-to-end check for a 404 on an SSE resume/listener GET.
//!
//! A local axum mock: `initialize` assigns `Mcp-Session-Id: session-1`; `ping`
//! answers with an SSE stream carrying one event id and then EOF, so the client
//! must resume with `GET` + `Mcp-Session-Id` + `Last-Event-ID` (SEP-1699). The
//! mock answers resume GETs with 404, exactly what rmcp's own server returns for
//! a terminated session.

#![cfg(all(
    feature = "client",
    feature = "transport-streamable-http-client",
    feature = "transport-streamable-http-client-reqwest",
    not(feature = "local")
))]

use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use axum::{
    Router,
    body::{Body, Bytes},
    response::Response,
    routing::any,
};
use http::{HeaderMap, HeaderValue, StatusCode, header::CONTENT_TYPE};
use rmcp::{
    RoleClient,
    model::{ClientConfig, ClientRequest, PingRequest},
    service::{ClientLifecycleMode, RunningService, serve_client_with_lifecycle},
    transport::{
        StreamableHttpClientTransport, common::client_side_sse::FixedInterval,
        streamable_http_client::StreamableHttpClientTransportConfig,
    },
};

const WAIT_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Default)]
struct AuditState {
    resume_gets: Arc<AtomicUsize>,
    listener_gets: Arc<AtomicUsize>,
    initializes: Arc<AtomicUsize>,
    reject_listener_get: bool,
}

fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

fn not_found() -> Response {
    let mut response = Response::new(Body::from("Not Found: Session not found"));
    *response.status_mut() = StatusCode::NOT_FOUND;
    response
}

fn open_event_stream() -> Response {
    let mut response = Response::new(Body::from_stream(futures::stream::pending::<
        Result<Bytes, std::io::Error>,
    >()));
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
    response
}

async fn handler(
    axum::extract::State(state): axum::extract::State<AuditState>,
    method: http::Method,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if method == http::Method::GET {
        if header_str(&headers, "last-event-id").is_some() {
            state.resume_gets.fetch_add(1, Ordering::SeqCst);
            return not_found();
        }
        state.listener_gets.fetch_add(1, Ordering::SeqCst);
        if state.reject_listener_get {
            return not_found();
        }
        return open_event_stream();
    }

    let value: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(_) => {
            let mut response = Response::new(Body::from("bad json"));
            *response.status_mut() = StatusCode::BAD_REQUEST;
            return response;
        }
    };
    match value["method"].as_str() {
        Some("initialize") => {
            state.initializes.fetch_add(1, Ordering::SeqCst);
            let body = serde_json::json!({
                "jsonrpc": "2.0",
                "id": value["id"].clone(),
                "result": {
                    "protocolVersion": "2025-11-25",
                    "capabilities": {},
                    "serverInfo": { "name": "audit", "version": "1" }
                }
            });
            let mut response = Response::new(Body::from(body.to_string()));
            response
                .headers_mut()
                .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
            response
                .headers_mut()
                .insert("mcp-session-id", HeaderValue::from_static("session-1"));
            response
        }
        Some("notifications/initialized") => {
            let mut response = Response::new(Body::empty());
            *response.status_mut() = StatusCode::ACCEPTED;
            response
        }
        Some("ping") => {
            let mut response = Response::new(Body::from("id: e1\n\n"));
            response
                .headers_mut()
                .insert(CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
            response
        }
        other => {
            let mut response = Response::new(Body::from(format!("unexpected {other:?}")));
            *response.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
            response
        }
    }
}

async fn start(
    state: &AuditState,
    reinit_on_expired_session: bool,
) -> anyhow::Result<(
    RunningService<RoleClient, ClientConfig>,
    tokio::task::JoinHandle<()>,
)> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    let addr = listener.local_addr()?;
    let server = tokio::spawn({
        let state = state.clone();
        async move {
            let app = Router::new().route("/mcp", any(handler)).with_state(state);
            axum::serve(listener, app).await.expect("mock server");
        }
    });
    let mut config = StreamableHttpClientTransportConfig::with_uri(format!("http://{addr}/mcp"));
    // Unbounded on purpose: only the terminal `SessionExpired` classification
    // stops the resume GET, not policy exhaustion.
    let mut retry = FixedInterval::default();
    retry.max_times = None;
    retry.duration = Duration::from_millis(1);
    config.retry_config = Arc::new(retry);
    config.reinit_on_expired_session = reinit_on_expired_session;
    let client = serve_client_with_lifecycle(
        ClientConfig::default(),
        StreamableHttpClientTransport::from_config(config),
        ClientLifecycleMode::Initialize,
    )
    .await?;
    Ok((client, server))
}

async fn wait_for(mut predicate: impl FnMut() -> bool) {
    tokio::time::timeout(WAIT_TIMEOUT, async {
        while !predicate() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("condition not reached");
}

async fn interrupted_ping(
    client: &RunningService<RoleClient, ClientConfig>,
) -> Result<rmcp::model::ServerResult, rmcp::ServiceError> {
    let peer = client.peer().clone();
    tokio::time::timeout(
        WAIT_TIMEOUT,
        peer.send_request(ClientRequest::PingRequest(PingRequest::default())),
    )
    .await
    .expect("ping must resolve instead of hanging on resume retries")
}

#[tokio::test]
async fn resume_get_404_fails_pending_request_with_session_expired() -> anyhow::Result<()> {
    let state = AuditState::default();
    let (client, server) = start(&state, false).await?;

    let error = interrupted_ping(&client)
        .await
        .expect_err("the interrupted request cannot be replayed");
    let resume_gets = state.resume_gets.load(Ordering::SeqCst);
    let initializes = state.initializes.load(Ordering::SeqCst);
    let _ = client.cancel().await;
    server.abort();

    assert!(error.to_string().contains("Session expired"), "{error}");
    assert_eq!(resume_gets, 1, "a 404 resume must not be retried");
    assert_eq!(initializes, 1, "reinitialization is disabled");
    Ok(())
}

#[tokio::test]
async fn resume_get_404_reinitializes_once_and_restarts_listener() -> anyhow::Result<()> {
    let state = AuditState::default();
    let (client, server) = start(&state, true).await?;

    let error = interrupted_ping(&client)
        .await
        .expect_err("the interrupted request cannot be replayed");
    wait_for(|| {
        state.initializes.load(Ordering::SeqCst) == 2
            && state.listener_gets.load(Ordering::SeqCst) == 2
    })
    .await;
    let resume_gets = state.resume_gets.load(Ordering::SeqCst);
    let initializes = state.initializes.load(Ordering::SeqCst);
    let listener_gets = state.listener_gets.load(Ordering::SeqCst);
    let _ = client.cancel().await;
    server.abort();

    assert!(error.to_string().contains("Session expired"), "{error}");
    assert_eq!(resume_gets, 1, "a 404 resume must not be retried");
    assert_eq!(initializes, 2, "exactly one re-initialization");
    assert_eq!(
        listener_gets, 2,
        "listener must restart on the replacement session"
    );
    Ok(())
}

#[tokio::test]
async fn initial_listener_get_404_reinitializes_at_most_once() -> anyhow::Result<()> {
    let state = AuditState {
        reject_listener_get: true,
        ..Default::default()
    };
    let (client, server) = start(&state, true).await?;

    wait_for(|| {
        state.initializes.load(Ordering::SeqCst) == 2
            && state.listener_gets.load(Ordering::SeqCst) == 2
    })
    .await;
    // One re-init per session_recovery_timeout: a server that 404s every
    // replacement listener must not drive an initialize hot loop.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let initializes = state.initializes.load(Ordering::SeqCst);
    let listener_gets = state.listener_gets.load(Ordering::SeqCst);
    let _ = client.cancel().await;
    server.abort();

    assert_eq!(initializes, 2, "only one recovery attempt is allowed");
    assert_eq!(listener_gets, 2, "the replacement listener 404 is terminal");
    Ok(())
}
