#![cfg(all(
    feature = "transport-streamable-http-server",
    feature = "transport-streamable-http-client-reqwest",
    not(feature = "local")
))]

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService,
    session::{SessionManager, local::LocalSessionManager},
};
use tokio_util::sync::CancellationToken;
use tracing_subscriber::layer::SubscriberExt;

mod common;
use common::calculator::Calculator;

struct CapturedEvent {
    level: tracing::Level,
    message: String,
}

struct CapturingLayer {
    events: Arc<Mutex<Vec<CapturedEvent>>>,
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for CapturingLayer {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let mut visitor = MessageVisitor(String::new());
        event.record(&mut visitor);
        self.events.lock().unwrap().push(CapturedEvent {
            level: *event.metadata().level(),
            message: visitor.0,
        });
    }
}

struct MessageVisitor(String);

impl tracing::field::Visit for MessageVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.0 = format!("{:?}", value);
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn test_keep_alive_timeout_does_not_emit_error_log() {
    let events = Arc::new(Mutex::new(Vec::<CapturedEvent>::new()));

    let subscriber = tracing_subscriber::registry().with(CapturingLayer {
        events: events.clone(),
    });

    let _guard = tracing::subscriber::set_default(subscriber);

    let ct = CancellationToken::new();
    let mut session_manager = LocalSessionManager::default();
    session_manager.session_config.keep_alive = Some(Duration::from_millis(200));
    let session_manager = Arc::new(session_manager);

    let service = StreamableHttpService::new(
        || Ok(Calculator::new()),
        session_manager.clone(),
        StreamableHttpServerConfig::default()
            .with_sse_keep_alive(None)
            .with_cancellation_token(ct.child_token()),
    );

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

    let response = client
        .post(format!("http://{addr}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .body(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let session_id = response.headers()["mcp-session-id"]
        .to_str()
        .unwrap()
        .to_string();

    client
        .post(format!("http://{addr}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("mcp-session-id", &session_id)
        .header("Mcp-Protocol-Version", "2025-06-18")
        .body(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
        .send()
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(400)).await;

    // Wait until close_session() has completed so all logs are captured.
    let session_id_parsed: Arc<str> = Arc::from(session_id.as_str());
    for _ in 0..20 {
        if !session_manager
            .has_session(&session_id_parsed)
            .await
            .unwrap()
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        !session_manager
            .has_session(&session_id_parsed)
            .await
            .unwrap(),
        "session should have been removed after idle reap"
    );

    let captured = events.lock().unwrap();

    let error_events: Vec<_> = captured
        .iter()
        .filter(|e| e.level == tracing::Level::ERROR)
        .collect();
    assert!(
        error_events.is_empty(),
        "idle reap should not produce any ERROR logs, found {}: {:?}",
        error_events.len(),
        error_events.iter().map(|e| &e.message).collect::<Vec<_>>()
    );

    let debug_events: Vec<_> = captured
        .iter()
        .filter(|e| e.level == tracing::Level::DEBUG && e.message.contains("IdleTimeout"))
        .collect();
    assert!(
        !debug_events.is_empty(),
        "expected a DEBUG log with IdleTimeout, but found none"
    );

    ct.cancel();
}

#[tokio::test(flavor = "current_thread")]
async fn test_explicit_close_on_live_session_succeeds() {
    let ct = CancellationToken::new();
    let mut session_manager = LocalSessionManager::default();
    session_manager.session_config.keep_alive = Some(Duration::from_secs(60));
    let session_manager = Arc::new(session_manager);

    let service = StreamableHttpService::new(
        || Ok(Calculator::new()),
        session_manager.clone(),
        StreamableHttpServerConfig::default()
            .with_sse_keep_alive(None)
            .with_cancellation_token(ct.child_token()),
    );

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

    let response = client
        .post(format!("http://{addr}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .body(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let session_id = response.headers()["mcp-session-id"]
        .to_str()
        .unwrap()
        .to_string();

    client
        .post(format!("http://{addr}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("mcp-session-id", &session_id)
        .header("Mcp-Protocol-Version", "2025-06-18")
        .body(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
        .send()
        .await
        .unwrap();

    let session_id_parsed: Arc<str> = Arc::from(session_id.as_str());

    assert!(
        session_manager
            .has_session(&session_id_parsed)
            .await
            .unwrap(),
        "session should exist before explicit close"
    );

    let result = session_manager.close_session(&session_id_parsed).await;
    assert!(
        result.is_ok(),
        "close_session on a live worker should succeed: {result:?}"
    );

    assert!(
        !session_manager
            .has_session(&session_id_parsed)
            .await
            .unwrap(),
        "session should not exist after explicit close"
    );

    ct.cancel();
}
