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
async fn default_client_truncates_error_body_without_exposing_the_tail() {
    const MARKER: &str = "TAIL_TEST_MARKER_DO_NOT_ECHO";
    let mut body = "x".repeat(65_536);
    body.push_str(MARKER);
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

    assert!(
        error.starts_with("unexpected server response: HTTP 500"),
        "{error}"
    );
    assert!(
        error.contains(&"x".repeat(65_536)),
        "expected diagnostic prefix"
    );
    assert!(
        !error.contains(MARKER),
        "error must not expose the discarded tail"
    );
    assert!(error.len() < 65_636, "error must remain bounded");
}
