#![cfg(all(
    feature = "transport-streamable-http-client",
    feature = "transport-streamable-http-client-reqwest",
    not(feature = "local")
))]

use std::{
    collections::HashMap,
    convert::Infallible,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    Router,
    body::{Body, Bytes},
    http::Response,
    routing::any,
};
use futures::{StreamExt, stream};
use rmcp::{
    ClientLifecycleMode, ClientServiceExt, RoleClient,
    model::{
        ClientConfig, ClientJsonRpcMessage, ClientRequest, PingRequest, ProtocolVersion, RequestId,
    },
    service::ClientInitializeError,
    transport::{
        StreamableHttpClientTransport,
        streamable_http_client::{
            StreamableHttpClient, StreamableHttpClientTransportConfig, StreamableHttpError,
        },
    },
};
use tokio::task::JoinHandle;

struct MockServer {
    uri: Arc<str>,
    task: JoinHandle<()>,
    methods: Arc<Mutex<Vec<String>>>,
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn mock_server(status: u16, unfinished: bool) -> MockServer {
    mock_response(status, unfinished, "private-response-body").await
}

async fn mock_response(status: u16, unfinished: bool, text: &'static str) -> MockServer {
    let methods = Arc::new(Mutex::new(Vec::new()));
    let captured = methods.clone();
    let router = Router::new().route(
        "/mcp",
        any(move |body: Bytes| {
            let methods = captured.clone();
            async move {
                if let Ok(message) = serde_json::from_slice::<serde_json::Value>(&body)
                    && let Some(method) = message["method"].as_str()
                {
                    methods.lock().unwrap().push(method.to_owned());
                }
                let body = if unfinished {
                    Body::from_stream(
                        stream::once(async move { Ok::<_, Infallible>(text) })
                            .chain(stream::pending()),
                    )
                } else {
                    Body::from(text)
                };
                Response::builder()
                    .status(status)
                    .header("retry-after", "17")
                    .header("content-type", "application/json")
                    .header("x-private-response", "private-response-header")
                    .body(body)
                    .unwrap()
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    MockServer {
        uri: Arc::from(format!("http://{address}/mcp?private-query")),
        task,
        methods,
    }
}

fn ping() -> ClientJsonRpcMessage {
    ClientJsonRpcMessage::request(
        ClientRequest::PingRequest(PingRequest::default()),
        RequestId::Number(1),
    )
}

fn assert_status_error(error: StreamableHttpError<reqwest::Error>, status: u16) {
    let rendered = format!("{error:?} {error}");
    assert!(
        !rendered.contains("private-"),
        "retained private response/request state: {rendered}"
    );
    match error {
        StreamableHttpError::HttpStatus(metadata) => {
            assert_eq!(metadata.status, status);
            assert_eq!(metadata.retry_after.as_deref(), Some("17"));
        }
        other => panic!("expected body-free HTTP status metadata, got {other:?}"),
    }
}

#[tokio::test]
async fn post_rate_limit_and_server_errors_return_before_the_body_finishes() {
    for status in [429, 500, 503, 599] {
        let server = mock_server(status, true).await;
        let client = reqwest::Client::new();
        let result = tokio::time::timeout(
            Duration::from_secs(2),
            client.post_message(
                server.uri.clone(),
                ping(),
                Some(Arc::from("private-session")),
                Some("private-token".to_owned()),
                HashMap::new(),
            ),
        )
        .await
        .expect("HTTP status classification must not wait for the error body to finish");
        assert_status_error(result.unwrap_err(), status);
    }
}

#[tokio::test]
async fn get_errors_keep_only_status_and_retry_after() {
    for status in [429, 503] {
        let server = mock_server(status, false).await;
        let error = reqwest::Client::new()
            .get_stream(
                server.uri.clone(),
                Some(Arc::from("private-session")),
                Some("private-event-id".to_owned()),
                Some("private-token".to_owned()),
                HashMap::new(),
            )
            .await
            .err()
            .expect("HTTP status must fail");
        assert_status_error(error, status);
    }
}

#[tokio::test]
async fn selected_status_takes_precedence_over_a_json_rpc_error_body() {
    // This is a deliberate difference from other HTTP errors (e.g. 400):
    // 429/5xx are classified without reading even a valid JSON-RPC error body.
    let text = r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32603,"message":"private-error"}}"#;
    let server = mock_response(503, false, text).await;
    let result = reqwest::Client::new()
        .post_message(server.uri.clone(), ping(), None, None, HashMap::new())
        .await;
    assert_status_error(result.unwrap_err(), 503);
}

#[tokio::test]
async fn rate_limited_auto_discovery_preserves_metadata_without_legacy_fallback() {
    let server = mock_server(429, false).await;
    let transport = StreamableHttpClientTransport::with_client(
        reqwest::Client::new(),
        StreamableHttpClientTransportConfig::with_uri(server.uri.clone()),
    );
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        ClientConfig::default().serve_with_lifecycle(
            transport,
            ClientLifecycleMode::Auto {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
                legacy_version: Some(ProtocolVersion::V_2025_11_25),
            },
        ),
    )
    .await
    .expect("rate-limited discovery should fail promptly");
    match result.expect_err("rate-limited discovery must fail") {
        ClientInitializeError::TransportError { error, .. } => {
            let error = error
                .downcast::<StreamableHttpClientTransport<reqwest::Client>, RoleClient>()
                .expect("transport error type must be preserved");
            assert_status_error(error, 429);
        }
        other => panic!("expected HTTP transport error, got {other:?}"),
    }
    assert_eq!(*server.methods.lock().unwrap(), ["server/discover"]);
}
