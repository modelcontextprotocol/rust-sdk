#![cfg(not(feature = "local"))]
use std::time::Duration;

use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService,
    session::{SessionId, local::LocalSessionManager},
};
use tokio_util::sync::CancellationToken;

mod common;
use common::calculator::Calculator;

#[tokio::test]
async fn test_priming_on_stream_start() -> anyhow::Result<()> {
    let ct = CancellationToken::new();

    // stateful_mode: true automatically enables priming with DEFAULT_RETRY_INTERVAL (3 seconds)
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
    // The cached events should be replayed and the stream should end.
    let resume_response = client
        .get(format!("http://{addr}/mcp"))
        .header("Accept", "text/event-stream")
        .header("mcp-session-id", session_id.to_string())
        .header("Mcp-Protocol-Version", "2025-06-18")
        .header("last-event-id", "0/0")
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await?;
    assert_eq!(resume_response.status(), 200);

    let resume_body = resume_response.text().await?;
    let resume_events: Vec<&str> = resume_body
        .split("\n\n")
        .filter(|e| !e.is_empty())
        .collect();
    assert!(
        !resume_events.is_empty(),
        "expected replayed events on resume, got empty"
    );

    // The replayed event should contain the original response
    let replayed = resume_events[0];
    assert!(
        replayed.contains(r#""id":2"#),
        "replayed event should contain the tool response, got: {replayed}"
    );

    ct.cancel();
    handle.await?;

    Ok(())
}

#[tokio::test]
async fn test_priming_on_stream_close() -> anyhow::Result<()> {
    use std::sync::Arc;

    use rmcp::transport::streamable_http_server::session::SessionId;

    let ct = CancellationToken::new();
    let session_manager = Arc::new(LocalSessionManager::default());

    // stateful_mode: true automatically enables priming with DEFAULT_RETRY_INTERVAL (3 seconds)
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
