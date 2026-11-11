#![cfg(all(
    feature = "transport-streamable-http-client",
    feature = "transport-streamable-http-client-reqwest",
    not(feature = "local")
))]

use std::{collections::HashMap, sync::Arc};

use rmcp::{
    model::{ClientJsonRpcMessage, ClientNotification, InitializedNotification},
    transport::streamable_http_client::{StreamableHttpClient, StreamableHttpPostResponse},
};

async fn spawn_server(router: axum::Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });

    format!("http://{addr}/mcp")
}

async fn post_initialized(
    url: &str,
) -> Result<
    StreamableHttpPostResponse,
    rmcp::transport::streamable_http_client::StreamableHttpError<reqwest::Error>,
> {
    reqwest::Client::new()
        .post_message(
            Arc::from(url),
            ClientJsonRpcMessage::notification(ClientNotification::InitializedNotification(
                InitializedNotification::default(),
            )),
            None,
            None,
            HashMap::new(),
        )
        .await
}

#[tokio::test]
async fn empty_success_response_to_notification_is_accepted() {
    use axum::{Router, http::StatusCode, routing::post};

    let url = spawn_server(Router::new().route("/mcp", post(|| async { StatusCode::OK }))).await;

    match post_initialized(&url).await {
        Ok(StreamableHttpPostResponse::Accepted) => {}
        other => panic!("expected Accepted, got: {other:?}"),
    }
}

#[tokio::test]
async fn success_response_with_unexpected_content_type_to_notification_is_accepted() {
    use axum::{Router, http::StatusCode, routing::post};

    let url = spawn_server(Router::new().route(
        "/mcp",
        post(|| async { (StatusCode::OK, "this is not a JSON-RPC message") }),
    ))
    .await;

    match post_initialized(&url).await {
        Ok(StreamableHttpPostResponse::Accepted) => {}
        other => panic!("expected Accepted, got: {other:?}"),
    }
}

#[tokio::test]
async fn success_response_without_content_type_or_length_to_notification_is_accepted() {
    use axum::{Router, body::Body, routing::post};

    // A streaming body of unknown length: no Content-Length, and no
    // Content-Type either, so the reply looks neither JSON nor SSE.
    let url = spawn_server(Router::new().route(
        "/mcp",
        post(|| async {
            Body::from_stream(futures::stream::empty::<
                Result<axum::body::Bytes, std::io::Error>,
            >())
        }),
    ))
    .await;

    match post_initialized(&url).await {
        Ok(StreamableHttpPostResponse::Accepted) => {}
        other => panic!("expected Accepted, got: {other:?}"),
    }
}

#[tokio::test]
async fn server_error_response_to_notification_is_still_rejected() {
    use axum::{Router, http::StatusCode, routing::post};
    use rmcp::transport::streamable_http_client::StreamableHttpError;

    // A non-2xx reply is an error even for a notification: the 2xx fallback
    // must stay below the status check, or server failures become accepted
    // sends.
    let url = spawn_server(Router::new().route(
        "/mcp",
        post(|| async { (StatusCode::INTERNAL_SERVER_ERROR, "server error") }),
    ))
    .await;

    match post_initialized(&url).await {
        Err(StreamableHttpError::HttpStatus(error)) => assert_eq!(error.status, 500),
        other => panic!("expected HttpStatus(500), got: {other:?}"),
    }
}

#[tokio::test]
async fn request_response_with_unexpected_content_type_is_still_rejected() {
    use axum::{Router, routing::post};
    use rmcp::{
        model::{ClientRequest, PingRequest, RequestId},
        transport::streamable_http_client::StreamableHttpError,
    };

    let url = spawn_server(
        Router::new().route("/mcp", post(|| async { "this is not a JSON-RPC message" })),
    )
    .await;

    let result = reqwest::Client::new()
        .post_message(
            Arc::from(url.as_str()),
            ClientJsonRpcMessage::request(
                ClientRequest::PingRequest(PingRequest::default()),
                RequestId::Number(1),
            ),
            None,
            None,
            HashMap::new(),
        )
        .await;

    match result {
        Err(StreamableHttpError::UnexpectedContentType(Some(content_type))) => {
            assert!(content_type.starts_with("text/plain"));
        }
        other => panic!("expected UnexpectedContentType, got: {other:?}"),
    }
}
