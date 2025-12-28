use std::time::Duration;

use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
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
            StreamableHttpServerConfig {
                stateful_mode: true,
                sse_keep_alive: None,
                cancellation_token: ct.child_token(),
                ..Default::default()
            },
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

    // Verify priming event (first event)
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
async fn test_priming_on_stream_close() -> anyhow::Result<()> {
    use std::sync::Arc;

    use rmcp::transport::streamable_http_server::session::SessionId;

    let ct = CancellationToken::new();
    let session_manager = Arc::new(LocalSessionManager::default());

    // stateful_mode: true automatically enables priming with DEFAULT_RETRY_INTERVAL (3 seconds)
    let service = StreamableHttpService::new(
        || Ok(Calculator::new()),
        session_manager.clone(),
        StreamableHttpServerConfig {
            stateful_mode: true,
            sse_keep_alive: None,
            cancellation_token: ct.child_token(),
            ..Default::default()
        },
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
