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
        ClientConfig, ClientJsonRpcMessage, ClientRequest, JsonRpcMessage, PingRequest,
        ProtocolVersion, RequestId,
    },
    service::ClientInitializeError,
    transport::{
        StreamableHttpClientTransport,
        streamable_http_client::{
            HttpStatusError, StreamableHttpClient, StreamableHttpClientTransportConfig,
            StreamableHttpError,
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
    mock_response(status, unfinished, "private-response-body", "text/plain").await
}

async fn mock_response(
    status: u16,
    unfinished: bool,
    text: &'static str,
    content_type: &'static str,
) -> MockServer {
    mock_response_with_challenge(status, unfinished, text, content_type, None).await
}

async fn mock_response_with_challenge(
    status: u16,
    unfinished: bool,
    text: &'static str,
    content_type: &'static str,
    challenge: Option<&'static str>,
) -> MockServer {
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
                let mut response = Response::builder()
                    .status(status)
                    .header("retry-after", "17")
                    .header("content-type", content_type)
                    .header("x-private-response", "private-response-header");
                if let Some(challenge) = challenge {
                    response = response.header("www-authenticate", challenge);
                }
                response.body(body).unwrap()
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

#[rstest::rstest]
#[case(500)]
#[case(503)]
#[tokio::test]
async fn json_rpc_errors_preserve_the_server_code_and_message(#[case] status: u16) {
    let text = r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32603,"message":"server-error"}}"#;
    let server = mock_response(status, false, text, "application/json").await;
    let result = reqwest::Client::new()
        .post_message(server.uri.clone(), ping(), None, None, HashMap::new())
        .await
        .expect("a JSON-RPC error should reach the caller");
    match result {
        rmcp::transport::streamable_http_client::StreamableHttpPostResponse::Json(
            JsonRpcMessage::Error(error),
            _,
        ) => {
            assert_eq!(error.id, Some(RequestId::Number(1)));
            assert_eq!(error.error.code.0, -32603);
            assert_eq!(error.error.message, "server-error");
        }
        other => panic!("expected the original JSON-RPC error, got {other:?}"),
    }
}

#[rstest::rstest]
#[case::unfinished(true)]
#[case::complete(false)]
#[tokio::test]
async fn json_rate_limits_return_without_reading_the_body(#[case] unfinished: bool) {
    let text = r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32603,"message":"private-error"}}"#;
    let server = mock_response(429, unfinished, text, "application/json").await;
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        reqwest::Client::new().post_message(server.uri.clone(), ping(), None, None, HashMap::new()),
    )
    .await
    .expect("JSON rate limits must not wait for the body to finish");
    assert_status_error(result.unwrap_err(), 429);
}

/// A 401 or 403 without a `WWW-Authenticate` challenge is settled by its
/// status alone, on GET and POST alike: body-free metadata, returned before
/// the body finishes, even when the body is a JSON-RPC error.
#[rstest::rstest]
#[tokio::test]
async fn challenge_less_auth_failures_return_body_free_metadata(
    #[values(401, 403)] status: u16,
    #[values(false, true)] get: bool,
    #[values(false, true)] json: bool,
    #[values(false, true)] unfinished: bool,
) {
    let (text, content_type) = if json {
        (
            r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32001,"message":"private-response-body"}}"#,
            "application/json",
        )
    } else {
        ("private-response-body", "text/plain")
    };
    let server = mock_response(status, unfinished, text, content_type).await;
    let client = reqwest::Client::new();
    let result = tokio::time::timeout(Duration::from_secs(2), async {
        if get {
            client
                .get_stream(
                    server.uri.clone(),
                    Some(Arc::from("private-session")),
                    None,
                    Some("private-token".to_owned()),
                    HashMap::new(),
                )
                .await
                .map(|_| ())
        } else {
            client
                .post_message(
                    server.uri.clone(),
                    ping(),
                    Some(Arc::from("private-session")),
                    Some("private-token".to_owned()),
                    HashMap::new(),
                )
                .await
                .map(|_| ())
        }
    })
    .await
    .expect("auth status classification must not wait for the body to finish");
    assert_status_error(result.unwrap_err(), status);
}

/// With a challenge, the specialized auth errors still win over status
/// metadata, and they too return before the body finishes.
#[rstest::rstest]
#[tokio::test]
async fn auth_challenges_take_precedence_over_status_metadata(
    #[values(401, 403)] status: u16,
    #[values(false, true)] get: bool,
) {
    let challenge = r#"Bearer error="insufficient_scope", scope="mcp:read""#;
    let server = mock_response_with_challenge(
        status,
        true,
        "private-response-body",
        "application/json",
        Some(challenge),
    )
    .await;
    let client = reqwest::Client::new();
    let error = tokio::time::timeout(Duration::from_secs(2), async {
        if get {
            client
                .get_stream(server.uri.clone(), None, None, None, HashMap::new())
                .await
                .map(|_| ())
        } else {
            client
                .post_message(server.uri.clone(), ping(), None, None, HashMap::new())
                .await
                .map(|_| ())
        }
    })
    .await
    .expect("auth challenges must not wait for the body")
    .expect_err("auth challenge must fail");
    match error {
        StreamableHttpError::AuthRequired(error) if status == 401 => {
            assert_eq!(error.www_authenticate_header, challenge);
        }
        StreamableHttpError::InsufficientScope(error) if status == 403 => {
            assert_eq!(error.www_authenticate_header, challenge);
            assert_eq!(error.required_scope.as_deref(), Some("mcp:read"));
        }
        other => panic!("expected specialized auth error, got {other:?}"),
    }
}

#[rstest::rstest]
#[case("private-invalid-json")]
#[case(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#)]
#[tokio::test]
async fn unusable_json_error_bodies_fall_back_to_status_metadata(#[case] text: &'static str) {
    let server = mock_response(503, false, text, "application/json").await;
    let result = reqwest::Client::new()
        .post_message(server.uri.clone(), ping(), None, None, HashMap::new())
        .await;
    assert_status_error(result.unwrap_err(), 503);
}

/// A challenge-less 401/403 or a 429 during auto discovery must not trigger
/// the legacy fallback, and its metadata must be reachable both by walking
/// the lifecycle error's source chain and by downcasting the transport error.
#[rstest::rstest]
#[case::unauthorized(401)]
#[case::forbidden(403)]
#[case::rate_limited(429)]
#[tokio::test]
async fn auto_discovery_preserves_metadata_without_legacy_fallback(#[case] status: u16) {
    let server = mock_server(status, true).await;
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
    .expect("HTTP failure during discovery should fail promptly");
    let error = result.expect_err("HTTP failure during discovery must fail");
    assert_eq!(error.is_authorization_required(), status == 401);
    assert_eq!(error.auth_challenge(), None);

    let mut source: &(dyn std::error::Error + 'static) = &error;
    let metadata = loop {
        let rendered = format!("{source:?} {source}");
        assert!(
            !rendered.contains("private-"),
            "lifecycle error retained private state: {rendered}"
        );
        if let Some(metadata) = source.downcast_ref::<HttpStatusError>() {
            break metadata;
        }
        source = source
            .source()
            .expect("HTTP status metadata must be reachable through the error's sources");
    };
    assert_eq!(metadata.status, status);
    assert_eq!(metadata.retry_after.as_deref(), Some("17"));

    match error {
        ClientInitializeError::TransportError { error, .. } => {
            let error = error
                .downcast::<StreamableHttpClientTransport<reqwest::Client>, RoleClient>()
                .expect("transport error type must be preserved");
            assert_status_error(error, status);
        }
        other => panic!("expected HTTP transport error, got {other:?}"),
    }
    assert_eq!(*server.methods.lock().unwrap(), ["server/discover"]);
}

#[test]
fn custom_backends_can_construct_bounded_status_metadata() {
    // This integration test is compiled outside rmcp, where a non-exhaustive
    // struct cannot be constructed with a struct literal.
    for value in [
        None,
        Some("17".to_owned()),
        Some("a".repeat(128)),
        Some("é".repeat(65)),
    ] {
        let expected = value.clone().filter(|value| value.len() <= 128);
        let error: StreamableHttpError<Infallible> = HttpStatusError::new(503, value).into();
        match error {
            StreamableHttpError::HttpStatus(metadata) => {
                assert_eq!(metadata.status, 503);
                assert_eq!(metadata.retry_after, expected);
            }
            other => panic!("expected custom backend status metadata, got {other:?}"),
        }
    }
}
