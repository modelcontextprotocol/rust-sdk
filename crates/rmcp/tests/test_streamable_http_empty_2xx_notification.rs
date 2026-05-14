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

async fn spawn_empty_ok_server() -> String {
    use axum::{Router, http::StatusCode, routing::post};

    let router = Router::new().route("/mcp", post(|| async { StatusCode::OK }));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });

    format!("http://{addr}/mcp")
}

#[tokio::test]
async fn empty_success_response_to_notification_is_accepted() {
    let url = spawn_empty_ok_server().await;
    let client = reqwest::Client::new();
    let result = client
        .post_message(
            Arc::from(url.as_str()),
            ClientJsonRpcMessage::notification(ClientNotification::InitializedNotification(
                InitializedNotification::default(),
            )),
            None,
            None,
            HashMap::new(),
        )
        .await;

    match result {
        Ok(StreamableHttpPostResponse::Accepted) => {}
        other => panic!("expected Accepted, got: {other:?}"),
    }
}
