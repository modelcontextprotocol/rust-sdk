#![cfg(all(
    not(feature = "local"),
    feature = "client",
    feature = "reqwest",
    feature = "transport-streamable-http-server"
))]

use std::{borrow::Cow, sync::Arc};

use axum::{
    Router,
    body::{Body, Bytes},
    extract::State,
    http::{Response, StatusCode},
    routing::post,
};
use rmcp::{
    ClientLifecycleMode, ClientServiceExt, ServerHandler,
    model::{ClientConfig, DiscoverResult, ErrorCode, ErrorData, ProtocolVersion},
    service::{ClientInitializeError, MaybeSendFuture, RequestContext, RoleClient, RoleServer},
    transport::{
        StreamableHttpClientTransport,
        streamable_http_client::{StreamableHttpClientTransportConfig, StreamableHttpError},
        streamable_http_server::{
            StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
        },
    },
};
use serde_json::json;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Default)]
struct DiscoverHttpServer;

impl ServerHandler for DiscoverHttpServer {
    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Borrowed(&[ProtocolVersion::V_2026_07_28])
    }
}

#[derive(Clone, Default)]
struct LegacyHttpServer;

impl ServerHandler for LegacyHttpServer {
    fn discover(
        &self,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<DiscoverResult, ErrorData>> + MaybeSendFuture + '_ {
        std::future::ready(Err(ErrorData::new(
            ErrorCode::METHOD_NOT_FOUND,
            "Method not found",
            None,
        )))
    }
}

#[derive(Clone, Default)]
struct PlainTextLegacyHttpState {
    methods: Arc<Mutex<Vec<String>>>,
    fail_initialized: bool,
}

async fn plain_text_legacy_http_handler(
    State(state): State<PlainTextLegacyHttpState>,
    body: Bytes,
) -> Response<Body> {
    let request: serde_json::Value = serde_json::from_slice(&body).expect("valid JSON-RPC body");
    let method = request["method"]
        .as_str()
        .expect("request method")
        .to_owned();
    state.methods.lock().await.push(method.clone());

    if method == "server/discover" {
        return Response::builder()
            .status(StatusCode::UNPROCESSABLE_ENTITY)
            .body(Body::from("Unexpected message, expect initialize request"))
            .expect("build rejection response");
    }

    if method == "initialize" {
        return Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "application/json")
            .body(Body::from(
                json!({
                    "jsonrpc": "2.0",
                    "id": request["id"],
                    "result": {
                        "protocolVersion": "2025-11-25",
                        "capabilities": {},
                        "serverInfo": {"name": "legacy", "version": "1.0"}
                    }
                })
                .to_string(),
            ))
            .expect("build initialize response");
    }

    if method == "notifications/initialized" && state.fail_initialized {
        return Response::builder()
            .status(StatusCode::TOO_MANY_REQUESTS)
            .header("retry-after", "17")
            .body(Body::from("rate limited"))
            .expect("build notification failure");
    }

    Response::builder()
        .status(StatusCode::ACCEPTED)
        .body(Body::empty())
        .expect("build notification response")
}

#[tokio::test]
async fn discover_http_client_bootstraps_headers_without_initialize() {
    let ct = CancellationToken::new();
    let service: StreamableHttpService<DiscoverHttpServer, LocalSessionManager> =
        StreamableHttpService::new(
            || Ok(DiscoverHttpServer),
            Default::default(),
            StreamableHttpServerConfig::default()
                .with_legacy_session_mode(false)
                .with_json_response(true)
                .with_cancellation_token(ct.child_token()),
        );
    let router = axum::Router::new().nest_service("/mcp", service);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener should bind");
    let address = listener.local_addr().expect("listener address");
    let server = tokio::spawn({
        let ct = ct.clone();
        async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async move { ct.cancelled_owned().await })
                .await;
        }
    });

    let transport = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(format!("http://{address}/mcp")),
    );
    let client = ClientConfig::default()
        .serve_with_lifecycle(
            transport,
            ClientLifecycleMode::Discover {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            },
        )
        .await
        .expect("discover HTTP client should start");
    client.list_tools(None).await.expect("list tools");
    client.cancel().await.expect("cancel client");

    ct.cancel();
    server.await.expect("server task");
}

#[tokio::test]
async fn auto_http_client_falls_back_to_stateful_legacy_startup() {
    let ct = CancellationToken::new();
    let service: StreamableHttpService<LegacyHttpServer, LocalSessionManager> =
        StreamableHttpService::new(
            || Ok(LegacyHttpServer),
            Default::default(),
            StreamableHttpServerConfig::default()
                .with_json_response(true)
                .with_cancellation_token(ct.child_token()),
        );
    let router = axum::Router::new().nest_service("/mcp", service);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener should bind");
    let address = listener.local_addr().expect("listener address");
    let server = tokio::spawn({
        let ct = ct.clone();
        async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async move { ct.cancelled_owned().await })
                .await;
        }
    });

    let transport = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(format!("http://{address}/mcp")),
    );
    let client = ClientConfig::default()
        .serve_with_lifecycle(
            transport,
            ClientLifecycleMode::Auto {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
                legacy_version: Some(ProtocolVersion::V_2025_11_25),
            },
        )
        .await
        .expect("auto HTTP client should fall back");
    client.list_tools(None).await.expect("list tools");
    client.cancel().await.expect("cancel client");

    ct.cancel();
    server.await.expect("server task");
}

#[tokio::test]
async fn auto_http_client_falls_back_after_plain_text_4xx_rejection() {
    let ct = CancellationToken::new();
    let state = PlainTextLegacyHttpState::default();
    let methods = state.methods.clone();
    let router = Router::new()
        .route("/mcp", post(plain_text_legacy_http_handler))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener should bind");
    let address = listener.local_addr().expect("listener address");
    let server = tokio::spawn({
        let ct = ct.clone();
        async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async move { ct.cancelled_owned().await })
                .await;
        }
    });

    let transport = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(format!("http://{address}/mcp")),
    );
    let client = ClientConfig::default()
        .serve_with_lifecycle(
            transport,
            ClientLifecycleMode::Auto {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
                legacy_version: Some(ProtocolVersion::V_2025_11_25),
            },
        )
        .await
        .expect("auto HTTP client should fall back after a transport-level rejection");
    client.cancel().await.expect("cancel client");

    assert_eq!(
        methods.lock().await.as_slice(),
        &["server/discover", "initialize", "notifications/initialized"]
    );
    ct.cancel();
    server.await.expect("server task");
}

#[rstest::rstest]
#[case::initialize(false)]
#[case::auto_fallback(true)]
#[tokio::test]
async fn initialized_notification_failure_preserves_typed_http_status(#[case] auto_fallback: bool) {
    let ct = CancellationToken::new();
    let state = PlainTextLegacyHttpState {
        methods: Arc::default(),
        fail_initialized: true,
    };
    let methods = state.methods.clone();
    let router = Router::new()
        .route("/mcp", post(plain_text_legacy_http_handler))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener should bind");
    let address = listener.local_addr().expect("listener address");
    let server = tokio::spawn({
        let ct = ct.clone();
        async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async move { ct.cancelled_owned().await })
                .await;
        }
    });

    let transport = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(format!("http://{address}/mcp")),
    );
    let lifecycle = if auto_fallback {
        ClientLifecycleMode::Auto {
            preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            legacy_version: Some(ProtocolVersion::V_2025_11_25),
        }
    } else {
        ClientLifecycleMode::Initialize
    };
    let error = ClientConfig::default()
        .serve_with_lifecycle(transport, lifecycle)
        .await
        .expect_err("initialized notification must fail");
    let error = match error {
        ClientInitializeError::LegacyFallbackFailed { fallback, .. } if auto_fallback => *fallback,
        error => error,
    };
    match error {
        ClientInitializeError::TransportError { error, .. } => {
            assert!(error.to_string().contains("HTTP 429"));
            // The borrowed accessor reaches the same typed error without
            // consuming it, and `startup_error()` unwraps the shared variant.
            let borrowed = error
                .downcast_ref::<StreamableHttpClientTransport<reqwest::Client>, RoleClient>()
                .expect("downcast_ref reaches the typed streamable error");
            match borrowed.startup_error() {
                StreamableHttpError::HttpStatus(metadata) => {
                    assert_eq!(metadata.status, 429);
                    assert_eq!(metadata.retry_after.as_deref(), Some("17"));
                }
                other => panic!("expected HTTP status via downcast_ref, got {other:?}"),
            }
            let error = error
                .downcast::<StreamableHttpClientTransport<reqwest::Client>, RoleClient>()
                .expect("typed streamable HTTP error");
            match error.startup_error() {
                StreamableHttpError::HttpStatus(metadata) => {
                    assert_eq!(metadata.status, 429);
                    assert_eq!(metadata.retry_after.as_deref(), Some("17"));
                }
                other => panic!("expected HTTP status, got {other:?}"),
            }
        }
        other => panic!("expected transport error, got {other:?}"),
    }
    let expected_methods: &[&str] = if auto_fallback {
        &["server/discover", "initialize", "notifications/initialized"]
    } else {
        &["initialize", "notifications/initialized"]
    };
    assert_eq!(methods.lock().await.as_slice(), expected_methods);
    ct.cancel();
    server.await.expect("server task");
}

#[derive(Clone, Default)]
struct VersionNegotiatingHttpState {
    methods: Arc<Mutex<Vec<String>>>,
}

/// Rejects the first probe with a 4xx `UnsupportedProtocolVersionError` carrying
/// an uncorrelated `id`, as a middleware rejection would. Accepts the retry.
async fn version_negotiating_http_handler(
    State(state): State<VersionNegotiatingHttpState>,
    body: Bytes,
) -> Response<Body> {
    let request: serde_json::Value = serde_json::from_slice(&body).expect("valid JSON-RPC body");
    let method = request["method"]
        .as_str()
        .expect("request method")
        .to_owned();
    state.methods.lock().await.push(method.clone());

    if method == "server/discover" {
        if state.methods.lock().await.len() == 1 {
            return Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "jsonrpc": "2.0",
                        "id": "server-error",
                        "error": {
                            "code": -32022,
                            "message": "Unsupported protocol version",
                            "data": {"supported": ["2025-11-25"]}
                        }
                    })
                    .to_string(),
                ))
                .expect("build version rejection response");
        }

        let result = DiscoverResult::new(vec![ProtocolVersion::V_2025_11_25], Default::default());
        return Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "application/json")
            .body(Body::from(
                json!({
                    "jsonrpc": "2.0",
                    "id": request["id"],
                    "result": result,
                })
                .to_string(),
            ))
            .expect("build discover response");
    }

    panic!("modern server must not be asked for {method}");
}

/// A recognized modern error in a 4xx discover rejection identifies a modern
/// server, so the client retries at a supported version instead of falling back
/// to legacy `initialize`.
#[tokio::test]
async fn auto_http_client_retries_version_after_modern_4xx_rejection() {
    let ct = CancellationToken::new();
    let state = VersionNegotiatingHttpState::default();
    let methods = state.methods.clone();
    let router = Router::new()
        .route("/mcp", post(version_negotiating_http_handler))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener should bind");
    let address = listener.local_addr().expect("listener address");
    let server = tokio::spawn({
        let ct = ct.clone();
        async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async move { ct.cancelled_owned().await })
                .await;
        }
    });

    let transport = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(format!("http://{address}/mcp")),
    );
    let client = ClientConfig::default()
        .serve_with_lifecycle(
            transport,
            ClientLifecycleMode::Auto {
                preferred_versions: vec![
                    ProtocolVersion::V_2026_07_28,
                    ProtocolVersion::V_2025_11_25,
                ],
                legacy_version: Some(ProtocolVersion::V_2025_11_25),
            },
        )
        .await
        .expect("auto HTTP client should retry discover at a supported version");
    client.cancel().await.expect("cancel client");

    assert_eq!(
        methods.lock().await.as_slice(),
        &["server/discover", "server/discover"],
        "expected a version-negotiation retry, not a legacy fallback"
    );
    ct.cancel();
    server.await.expect("server task");
}
