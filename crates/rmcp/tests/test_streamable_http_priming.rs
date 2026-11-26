#![cfg(not(feature = "local"))]
use std::{sync::Arc, time::Duration};

use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ServerCapabilities,
        ServerConfig,
    },
    service::RequestContext,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService,
        session::{SessionId, local::LocalSessionManager},
    },
};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

mod common;
use common::calculator::Calculator;

#[tokio::test]
async fn test_priming_on_stream_start() -> anyhow::Result<()> {
    let ct = CancellationToken::new();

    // legacy_session_mode: true automatically enables priming with DEFAULT_RETRY_INTERVAL (3 seconds)
    let service: StreamableHttpService<Calculator, LocalSessionManager> =
        StreamableHttpService::new(
            || Ok(Calculator::new()),
            Default::default(),
            StreamableHttpServerConfig::default()
                .with_sse_keep_alive(None)
                .with_cancellation_token(ct.child_token()),
        );

    let router = axum::Router::new().nest_service("/mcp", service);
    let tcp_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = tcp_listener.local_addr()?;

    let handle = tokio::spawn({
        let ct = ct.clone();
        async move {
            let _ = axum::serve(tcp_listener, router)
                .with_graceful_shutdown(async move { ct.cancelled_owned().await })
                .await;
        }
    });

    // Send initialize request
    let client = reqwest::Client::new();
    let response = client
        .post(format!("http://{addr}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .body(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}"#)
        .send()
        .await?;

    assert_eq!(response.status(), 200);

    let body = response.text().await?;

    // Split SSE events by double newline
    let events: Vec<&str> = body.split("\n\n").filter(|e| !e.is_empty()).collect();
    assert!(events.len() >= 2);

    // Verify priming event (first event) — initialize uses "0" (no http_request_id)
    let priming_event = events[0];
    assert!(priming_event.contains("id: 0"));
    assert!(priming_event.contains("retry: 3000"));
    assert!(priming_event.contains("data:"));

    // Verify initialize response (second event)
    let response_event = events[1];
    assert!(response_event.contains(r#""jsonrpc":"2.0""#));
    assert!(response_event.contains(r#""id":1"#));

    ct.cancel();
    handle.await?;

    Ok(())
}

#[tokio::test]
async fn test_request_wise_priming_includes_http_request_id() -> anyhow::Result<()> {
    let ct = CancellationToken::new();

    let service: StreamableHttpService<Calculator, LocalSessionManager> =
        StreamableHttpService::new(
            || Ok(Calculator::new()),
            Default::default(),
            StreamableHttpServerConfig::default()
                .with_sse_keep_alive(None)
                .with_cancellation_token(ct.child_token()),
        );

    let router = axum::Router::new().nest_service("/mcp", service);
    let tcp_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = tcp_listener.local_addr()?;

    let handle = tokio::spawn({
        let ct = ct.clone();
        async move {
            let _ = axum::serve(tcp_listener, router)
                .with_graceful_shutdown(async move { ct.cancelled_owned().await })
                .await;
        }
    });

    let client = reqwest::Client::new();

    // Initialize the session
    let response = client
        .post(format!("http://{addr}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .body(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}"#)
        .send()
        .await?;
    assert_eq!(response.status(), 200);
    let session_id: SessionId = response.headers()["mcp-session-id"].to_str()?.into();

    // Send notifications/initialized
    let status = client
        .post(format!("http://{addr}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("mcp-session-id", session_id.to_string())
        .header("Mcp-Protocol-Version", "2025-06-18")
        .body(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
        .send()
        .await?
        .status();
    assert_eq!(status, 202);

    // First tool call — should get http_request_id 0
    let body = client
        .post(format!("http://{addr}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("mcp-session-id", session_id.to_string())
        .header("Mcp-Protocol-Version", "2025-06-18")
        .body(r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"sum","arguments":{"a":1,"b":2}}}"#)
        .send()
        .await?
        .text()
        .await?;

    let events: Vec<&str> = body.split("\n\n").filter(|e| !e.is_empty()).collect();
    assert!(
        events.len() >= 2,
        "expected priming + response, got: {body}"
    );

    // Priming event should encode the http_request_id (0)
    let priming = events[0];
    assert!(
        priming.contains("id: 0/0"),
        "first request priming should be 0/0, got: {priming}"
    );
    assert!(priming.contains("retry: 3000"));

    // Response event should use index 1 (since priming occupies index 0)
    let response_event = events[1];
    assert!(
        response_event.contains("id: 1/0"),
        "first response event id should be 1/0, got: {response_event}"
    );
    assert!(response_event.contains(r#""id":2"#));

    // Second tool call — should get http_request_id 1
    let body = client
        .post(format!("http://{addr}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("mcp-session-id", session_id.to_string())
        .header("Mcp-Protocol-Version", "2025-06-18")
        .body(r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"sum","arguments":{"a":3,"b":4}}}"#)
        .send()
        .await?
        .text()
        .await?;

    let events: Vec<&str> = body.split("\n\n").filter(|e| !e.is_empty()).collect();
    assert!(
        events.len() >= 2,
        "expected priming + response, got: {body}"
    );

    let priming = events[0];
    assert!(
        priming.contains("id: 0/1"),
        "second request priming should be 0/1, got: {priming}"
    );

    let response_event = events[1];
    assert!(
        response_event.contains("id: 1/1"),
        "second response event id should be 1/1, got: {response_event}"
    );
    assert!(response_event.contains(r#""id":3"#));

    ct.cancel();
    handle.await?;

    Ok(())
}

#[tokio::test]
async fn test_resume_after_request_wise_channel_completed() -> anyhow::Result<()> {
    let ct = CancellationToken::new();

    let service: StreamableHttpService<Calculator, LocalSessionManager> =
        StreamableHttpService::new(
            || Ok(Calculator::new()),
            Default::default(),
            StreamableHttpServerConfig::default()
                .with_sse_keep_alive(None)
                .with_cancellation_token(ct.child_token()),
        );

    let router = axum::Router::new().nest_service("/mcp", service);
    let tcp_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = tcp_listener.local_addr()?;

    let handle = tokio::spawn({
        let ct = ct.clone();
        async move {
            let _ = axum::serve(tcp_listener, router)
                .with_graceful_shutdown(async move { ct.cancelled_owned().await })
                .await;
        }
    });

    let client = reqwest::Client::new();

    // Initialize session
    let response = client
        .post(format!("http://{addr}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .body(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}"#)
        .send()
        .await?;
    assert_eq!(response.status(), 200);
    let session_id: SessionId = response.headers()["mcp-session-id"].to_str()?.into();

    // Complete handshake
    let status = client
        .post(format!("http://{addr}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("mcp-session-id", session_id.to_string())
        .header("Mcp-Protocol-Version", "2025-06-18")
        .body(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
        .send()
        .await?
        .status();
    assert_eq!(status, 202);

    // Call a tool and consume the full response (channel completes)
    let body = client
        .post(format!("http://{addr}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("mcp-session-id", session_id.to_string())
        .header("Mcp-Protocol-Version", "2025-06-18")
        .body(r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"sum","arguments":{"a":1,"b":2}}}"#)
        .send()
        .await?
        .text()
        .await?;

    let events: Vec<&str> = body.split("\n\n").filter(|e| !e.is_empty()).collect();
    assert!(
        events.len() >= 2,
        "expected priming + response, got: {body}"
    );
    assert!(events[0].contains("id: 0/0"));
    assert!(events[1].contains(r#""id":2"#));

    // Resume with Last-Event-ID after the channel has completed.
    // The completed request-wise channel retains its cache within
    // `completed_cache_ttl` and replays missed events before ending.
    let resume = client
        .get(format!("http://{addr}/mcp"))
        .header("Accept", "text/event-stream")
        .header("mcp-session-id", session_id.to_string())
        .header("Mcp-Protocol-Version", "2025-06-18")
        .header("last-event-id", "0/0")
        .send()
        .await?;
    assert_eq!(resume.status(), 200);

    let resume_body = resume.text().await?;
    assert!(
        resume_body.contains("id: 1/0") && resume_body.contains(r#""id":2"#),
        "expected replayed tool response in resume stream, got: {resume_body}"
    );
    assert!(
        !resume_body.contains("standalone"),
        "should not receive events from a different stream"
    );

    ct.cancel();
    handle.await?;

    Ok(())
}

#[tokio::test]
async fn test_completed_cache_ttl_eviction() -> anyhow::Result<()> {
    let ct = CancellationToken::new();
    let mut session_manager = LocalSessionManager::default();
    session_manager.session_config.completed_cache_ttl = Duration::from_millis(200);
    let session_manager = Arc::new(session_manager);

    let service = StreamableHttpService::new(
        || Ok(Calculator::new()),
        session_manager.clone(),
        StreamableHttpServerConfig::default()
            .with_sse_keep_alive(None)
            .with_cancellation_token(ct.child_token()),
    );

    let router = axum::Router::new().nest_service("/mcp", service);
    let tcp_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = tcp_listener.local_addr()?;

    let handle = tokio::spawn({
        let ct = ct.clone();
        async move {
            let _ = axum::serve(tcp_listener, router)
                .with_graceful_shutdown(async move { ct.cancelled_owned().await })
                .await;
        }
    });

    let client = reqwest::Client::new();

    // Initialize session
    let response = client
        .post(format!("http://{addr}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .body(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}"#)
        .send()
        .await?;
    assert_eq!(response.status(), 200);
    let session_id: SessionId = response.headers()["mcp-session-id"].to_str()?.into();

    // Complete handshake
    client
        .post(format!("http://{addr}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("mcp-session-id", session_id.to_string())
        .header("Mcp-Protocol-Version", "2025-06-18")
        .body(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
        .send()
        .await?;

    // Call a tool and consume the response (channel completes)
    let body = client
        .post(format!("http://{addr}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("mcp-session-id", session_id.to_string())
        .header("Mcp-Protocol-Version", "2025-06-18")
        .body(r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"sum","arguments":{"a":1,"b":2}}}"#)
        .send()
        .await?
        .text()
        .await?;
    assert!(body.contains(r#""id":2"#));

    // Resume immediately (within TTL) — cached response must still be replayed
    let pre_eviction = client
        .get(format!("http://{addr}/mcp"))
        .header("Accept", "text/event-stream")
        .header("mcp-session-id", session_id.to_string())
        .header("Mcp-Protocol-Version", "2025-06-18")
        .header("last-event-id", "0/0")
        .send()
        .await?;
    assert_eq!(pre_eviction.status(), 200);
    let pre_eviction_body = pre_eviction.text().await?;
    assert!(
        pre_eviction_body.contains("id: 1/0") && pre_eviction_body.contains(r#""id":2"#),
        "expected cached response before TTL expiry, got: {pre_eviction_body}"
    );

    // Wait for TTL to expire (200ms) plus margin
    tokio::time::sleep(Duration::from_millis(400)).await;

    // Resume directly after TTL without sending other session events — resume()
    // must evict expired entries and return 200 with an empty stream.
    let resume = client
        .get(format!("http://{addr}/mcp"))
        .header("Accept", "text/event-stream")
        .header("mcp-session-id", session_id.to_string())
        .header("Mcp-Protocol-Version", "2025-06-18")
        .header("last-event-id", "0/0")
        .send()
        .await?;
    assert_eq!(resume.status(), 200);

    let body = resume.text().await?;
    assert!(
        !body.contains(r#""id":2"#),
        "should NOT contain the old tool response after eviction, got: {body}"
    );

    ct.cancel();
    handle.await?;

    Ok(())
}

#[derive(Clone)]
struct SlowToolServer {
    started: Arc<Notify>,
    release: Arc<Notify>,
}

impl ServerHandler for SlowToolServer {
    #[allow(deprecated)]
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
    }

    async fn call_tool(
        &self,
        _request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        self.started.notify_one();
        self.release.notified().await;
        Ok(CallToolResult::success(vec![ContentBlock::text("slow-ok")]).into())
    }
}

#[tokio::test]
async fn test_resume_after_close_sse_stream_replays_completed_response() -> anyhow::Result<()> {
    let ct = CancellationToken::new();
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let server = SlowToolServer {
        started: started.clone(),
        release: release.clone(),
    };

    let session_manager = Arc::new(LocalSessionManager::default());
    let service = StreamableHttpService::new(
        move || Ok(server.clone()),
        session_manager.clone(),
        StreamableHttpServerConfig::default()
            .with_sse_keep_alive(None)
            .with_cancellation_token(ct.child_token()),
    );

    let router = axum::Router::new().nest_service("/mcp", service);
    let tcp_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = tcp_listener.local_addr()?;

    let handle = tokio::spawn({
        let ct = ct.clone();
        async move {
            let _ = axum::serve(tcp_listener, router)
                .with_graceful_shutdown(async move { ct.cancelled_owned().await })
                .await;
        }
    });

    let client = reqwest::Client::new();
    let url = format!("http://{addr}/mcp");

    // Initialize session
    let response = client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .body(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}"#)
        .send()
        .await?;
    assert_eq!(response.status(), 200);
    let session_id: SessionId = response.headers()["mcp-session-id"].to_str()?.into();

    let status = client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("mcp-session-id", session_id.to_string())
        .header("Mcp-Protocol-Version", "2025-06-18")
        .body(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
        .send()
        .await?
        .status();
    assert_eq!(status, 202);

    // Start slow tool call in the background
    let post_handle = tokio::spawn({
        let client = client.clone();
        let url = url.clone();
        let session_id = session_id.clone();
        async move {
            let resp = client
                .post(&url)
                .header("Content-Type", "application/json")
                .header("Accept", "application/json, text/event-stream")
                .header("mcp-session-id", session_id.to_string())
                .header("Mcp-Protocol-Version", "2025-06-18")
                .body(r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"slow","arguments":{}}}"#)
                .send()
                .await?;
            anyhow::Ok((resp.status(), resp.text().await?))
        }
    });

    started.notified().await;

    let session = session_manager
        .sessions
        .read()
        .await
        .get(&session_id)
        .expect("session exists")
        .clone();

    // Server-initiated SSE disconnect while request is still in-flight
    session
        .close_sse_stream(0, Some(Duration::from_millis(50)))
        .await?;

    let (post_status, post_body) = post_handle.await??;
    assert_eq!(post_status, 200);
    assert!(
        post_body.contains("id: 0/0")
            && post_body.contains("id: 1/0")
            && post_body.contains("retry: 50"),
        "expected initial + close priming events before disconnect, got: {post_body}"
    );
    assert!(
        !post_body.contains(r#""id":2"#),
        "response should not have arrived before disconnect, got: {post_body}"
    );

    // Release the handler so the response completes into the retained cache
    // before the client reconnects with Last-Event-ID.
    release.notify_one();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if session.close_sse_stream(0, None).await.is_err() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await?;

    let resume = client
        .get(&url)
        .header("Accept", "text/event-stream")
        .header("mcp-session-id", session_id.to_string())
        .header("Mcp-Protocol-Version", "2025-06-18")
        .header("last-event-id", "1/0")
        .send()
        .await?;
    assert_eq!(resume.status(), 200);

    let resume_body = resume.text().await?;
    assert!(
        resume_body.contains("id: 2/0")
            && resume_body.contains(r#""id":2"#)
            && resume_body.contains("slow-ok"),
        "expected completed response replayed on resume after disconnect, got: {resume_body}"
    );

    ct.cancel();
    handle.await?;

    Ok(())
}

#[tokio::test]
async fn test_priming_on_stream_close() -> anyhow::Result<()> {
    let ct = CancellationToken::new();
    let session_manager = Arc::new(LocalSessionManager::default());

    // legacy_session_mode: true automatically enables priming with DEFAULT_RETRY_INTERVAL (3 seconds)
    let service = StreamableHttpService::new(
        || Ok(Calculator::new()),
        session_manager.clone(),
        StreamableHttpServerConfig::default()
            .with_sse_keep_alive(None)
            .with_cancellation_token(ct.child_token()),
    );

    let router = axum::Router::new().nest_service("/mcp", service);
    let tcp_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = tcp_listener.local_addr()?;

    let handle = tokio::spawn({
        let ct = ct.clone();
        async move {
            let _ = axum::serve(tcp_listener, router)
                .with_graceful_shutdown(async move { ct.cancelled_owned().await })
                .await;
        }
    });

    // Send initialize request to create a session
    let client = reqwest::Client::new();
    let response = client
        .post(format!("http://{addr}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .body(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}"#)
        .send()
        .await?;

    let session_id: SessionId = response.headers()["mcp-session-id"].to_str()?.into();

    // Open a standalone GET stream (send() returns when headers are received)
    let response = client
        .get(format!("http://{addr}/mcp"))
        .header("Accept", "text/event-stream")
        .header("mcp-session-id", session_id.to_string())
        .send()
        .await?;

    assert_eq!(response.status(), 200);

    // Spawn a task to read the response body (blocks until stream closes)
    let read_task = tokio::spawn(async move { response.text().await.unwrap() });

    // Close the standalone stream with a 5-second retry hint
    let sessions = session_manager.sessions.read().await;
    let session = sessions.get(&session_id).unwrap();
    session
        .close_standalone_sse_stream(Some(Duration::from_secs(5)))
        .await?;
    drop(sessions);

    // Wait for the read task to complete and verify the response
    let body = read_task.await?;

    // Verify the stream received two priming events:
    // 1. At stream start (retry: 3000)
    // 2. Before close (retry: 5000)
    let events: Vec<&str> = body.split("\n\n").filter(|e| !e.is_empty()).collect();
    assert_eq!(events.len(), 2);

    // First event: priming at stream start
    let start_priming = events[0];
    assert!(start_priming.contains("id:"));
    assert!(start_priming.contains("retry: 3000"));
    assert!(start_priming.contains("data:"));

    // Second event: priming before close
    let close_priming = events[1];
    assert!(close_priming.contains("id:"));
    assert!(close_priming.contains("retry: 5000"));
    assert!(close_priming.contains("data:"));

    ct.cancel();
    handle.await?;

    Ok(())
}
