#![cfg(all(
    feature = "transport-streamable-http-client",
    feature = "transport-streamable-http-client-reqwest",
    feature = "transport-streamable-http-server"
))]

use std::{collections::HashMap, sync::Arc};

use rmcp::{
    model::{ClientJsonRpcMessage, ClientRequest, PingRequest, RequestId},
    transport::{
        streamable_http_client::{StreamableHttpClient, StreamableHttpError},
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
        Err(StreamableHttpError::UnexpectedServerResponse(message)) => {
            let message = message.to_string();
            assert!(
                message.contains("404"),
                "error should include HTTP status code, got: {message}"
            );
            assert!(
                message.to_ascii_lowercase().contains("session not found"),
                "error should include session-not-found hint, got: {message}"
            );
        }
        other => panic!("expected UnexpectedServerResponse, got: {other:?}"),
    }

    ct.cancel();
    handle.await?;

    Ok(())
}
