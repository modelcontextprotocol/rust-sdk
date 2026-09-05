#![cfg(all(
    feature = "client",
    feature = "transport-streamable-http-client-reqwest",
    not(feature = "local")
))]

use std::{collections::HashMap, sync::Arc, time::Duration};

use axum::{Router, http::StatusCode, routing::post};
use rmcp::{
    model::{ClientJsonRpcMessage, ClientRequest, PingRequest, RequestId},
    transport::streamable_http_client::StreamableHttpClient,
};

struct MockServer {
    uri: Arc<str>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl MockServer {
    async fn start(body: String) -> Self {
        let router = Router::new().route(
            "/mcp",
            post(move || {
                let body = body.clone();
                async move { (StatusCode::INTERNAL_SERVER_ERROR, body) }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Self {
            uri: Arc::from(format!("http://{address}/mcp")),
            task,
        }
    }
}

/// Exercise only the existing public API so this regression also compiles
/// against an SDK that still buffers HTTP error responses without a bound.
#[tokio::test]
async fn default_client_rejects_oversized_error_without_exposing_body() {
    const MARKER: &str = "SENSITIVE_TEST_MARKER_DO_NOT_ECHO";
    let mut body = MARKER.to_owned();
    body.push_str(&"x".repeat(65_537 - MARKER.len()));
    assert_eq!(body.len(), 65_537);
    let server = MockServer::start(body).await;
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let message = ClientJsonRpcMessage::request(
        ClientRequest::PingRequest(PingRequest::default()),
        RequestId::Number(1),
    );

    let error = client
        .post_message(server.uri.clone(), message, None, None, HashMap::new())
        .await
        .expect_err("an oversized HTTP error must fail")
        .to_string();

    assert!(error.contains("exceeded"), "expected a size-limit error");
    assert!(
        !error.contains(MARKER),
        "error must not expose response body"
    );
    assert!(error.len() < 512, "error must remain bounded");
}
