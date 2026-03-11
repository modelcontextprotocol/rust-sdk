#![cfg(all(
    feature = "transport-streamable-http-client",
    feature = "transport-streamable-http-client-reqwest",
    feature = "transport-streamable-http-server"
))]

use std::{collections::HashMap, sync::Arc};

use rmcp::{
    ServiceExt,
    model::{ClientJsonRpcMessage, ClientRequest, PingRequest, RequestId},
    transport::{
        StreamableHttpClientTransport,
        streamable_http_client::{
            StreamableHttpClient, StreamableHttpClientTransportConfig, StreamableHttpError,
        },
        streamable_http_server::{
            StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
        },
    },
};
use tokio_util::sync::CancellationToken;

mod common;
use common::calculator::Calculator;

#[tokio::test]
async fn test_stale_session_id_returns_status_aware_error() -> anyhow::Result<()> {
    let ct = CancellationToken::new();
    let (service, http_work): (StreamableHttpService<Calculator, LocalSessionManager>, _) =
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
    tokio::spawn(http_work);

    let router = axum::Router::new().nest_service("/mcp", service);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;

    let handle = tokio::spawn({
        let ct = ct.clone();
        async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async move { ct.cancelled_owned().await })
                .await;
        }
    });

    let uri = Arc::<str>::from(format!("http://{addr}/mcp"));
    let message = ClientJsonRpcMessage::request(
        ClientRequest::PingRequest(PingRequest::default()),
        RequestId::Number(1),
    );

    let client = reqwest::Client::new();
    let result = client
        .post_message(
            uri.clone(),
            message,
            Some(Arc::from("stale-session-id")),
            None,
            HashMap::new(),
        )
        .await;

    let raw_response = reqwest::Client::new()
        .post(uri.as_ref())
        .header("accept", "application/json, text/event-stream")
        .header("content-type", "application/json")
        .header("mcp-session-id", "stale-session-id")
        .body(r#"{"jsonrpc":"2.0","id":1,"method":"ping","params":{}}"#)
        .send()
        .await?;

    assert_eq!(raw_response.status(), reqwest::StatusCode::NOT_FOUND);
    match result {
        Err(StreamableHttpError::SessionExpired) => {
            // Expected: post_message detects 404 with a session ID and returns SessionExpired
        }
        other => panic!("expected SessionExpired, got: {other:?}"),
    }

    ct.cancel();
    handle.await?;

    Ok(())
}

/// Verify that when the server loses a session (returns HTTP 404), the client
/// transparently re-initializes and the original request succeeds.
#[tokio::test]
async fn test_transparent_reinitialization_on_session_expiry() -> anyhow::Result<()> {
    let ct = CancellationToken::new();
    let session_manager = Arc::new(LocalSessionManager::default());

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
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;

    let server_handle = tokio::spawn({
        let ct = ct.clone();
        async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async move { ct.cancelled_owned().await })
                .await;
        }
    });

    // Connect a full client transport (this performs initialize + notifications/initialized)
    let transport = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(format!("http://{addr}/mcp")),
    );
    let client = ().serve(transport).await?;

    // Verify the session is established: list_all_resources() succeeds
    let _resources = client.list_all_resources().await?;

    // Capture the current session ID from the server
    let original_session_id = {
        let sessions = session_manager.sessions.read().await;
        sessions
            .keys()
            .next()
            .cloned()
            .expect("session should exist")
    };

    // Force session expiry by removing all sessions from the server-side manager
    {
        let mut sessions = session_manager.sessions.write().await;
        sessions.clear();
    }

    // This call should trigger transparent re-initialization and still succeed
    let _resources_after = client.list_all_resources().await?;

    // Verify the server created a new session with a different ID
    {
        let sessions = session_manager.sessions.read().await;
        let new_session_id = sessions
            .keys()
            .next()
            .expect("new session should exist after re-initialization");
        assert_ne!(
            new_session_id, &original_session_id,
            "new session ID should differ from the original"
        );
    }

    let _ = client.cancel().await;
    ct.cancel();
    server_handle.await?;

    Ok(())
}
