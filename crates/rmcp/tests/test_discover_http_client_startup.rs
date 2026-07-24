#![cfg(all(
    not(feature = "local"),
    feature = "client",
    feature = "reqwest",
    feature = "transport-streamable-http-server"
))]

use std::{
    borrow::Cow,
    collections::HashMap,
    sync::{Arc, Mutex},
};

use axum::{
    body::Bytes,
    extract::State,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::post,
};
use futures::{StreamExt, stream};
use http::{HeaderName, HeaderValue};
use rmcp::{
    ClientLifecycleMode, ClientServiceExt, ServerHandler,
    model::{
        ClientInfo, ClientJsonRpcMessage, ClientRequest, DiscoverResult, ErrorCode, ErrorData,
        Implementation, InitializeResult, ProtocolVersion, ServerCapabilities,
        ServerJsonRpcMessage, ServerResult,
    },
    service::{MaybeSendFuture, RequestContext, RoleServer},
    transport::{
        StreamableHttpClientTransport,
        streamable_http_client::{
            HttpStatusError, StreamableHttpClient, StreamableHttpClientTransportConfig,
            StreamableHttpError, StreamableHttpPostResponse,
        },
        streamable_http_server::{
            StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
        },
    },
};
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
    let client = ClientInfo::default()
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
    let client = ClientInfo::default()
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

#[derive(Debug, Clone, Copy)]
enum LegacyDiscoveryRejection {
    UnsupportedProtocol,
    UnsupportedProtocolWithoutVersionList,
    MissingSession,
    InvalidRequest,
    InvalidParams,
    NotFound,
    MethodNotAllowed,
    Unauthorized,
    Forbidden,
    UnauthorizedJson,
    ForbiddenJson,
    UnrelatedResponseId,
    NotFoundWithUnrelatedResponseId,
    ArbitraryBadRequest,
    MixedFutureVersions,
    InternalServerErrorWithLegacyBody,
}

#[derive(Clone)]
struct LegacyPrevalidationState {
    rejection: LegacyDiscoveryRejection,
    methods: Arc<Mutex<Vec<String>>>,
}

async fn legacy_prevalidation_handler(
    State(state): State<LegacyPrevalidationState>,
    body: Bytes,
) -> Response {
    let message: serde_json::Value = serde_json::from_slice(&body).expect("JSON-RPC request body");
    let method = message
        .get("method")
        .and_then(serde_json::Value::as_str)
        .expect("JSON-RPC request method");
    state
        .methods
        .lock()
        .expect("methods lock")
        .push(method.into());

    match method {
        "server/discover" => match state.rejection {
            LegacyDiscoveryRejection::UnsupportedProtocol => json_response(
                StatusCode::BAD_REQUEST,
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": null,
                    "error": {
                        "code": -32000,
                        "message": "Bad Request: Unsupported protocol version: 2026-07-28 \
                            (supported versions: 2025-11-25, 2025-06-18, 2025-03-26, \
                            2024-11-05, 2024-10-07)",
                    },
                }),
            ),
            LegacyDiscoveryRejection::UnsupportedProtocolWithoutVersionList => json_response(
                StatusCode::BAD_REQUEST,
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": null,
                    "error": {
                        "code": -32000,
                        "message": "Bad Request: Unsupported protocol version: 2026-07-28",
                    },
                }),
            ),
            LegacyDiscoveryRejection::MissingSession => json_response(
                StatusCode::BAD_REQUEST,
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": null,
                    "error": {
                        "code": -32000,
                        "message": "Bad Request: No valid session ID provided",
                    },
                }),
            ),
            LegacyDiscoveryRejection::InvalidRequest => json_response(
                StatusCode::BAD_REQUEST,
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": message.get("id"),
                    "error": {
                        "code": -32600,
                        "message": "Invalid Request",
                    },
                }),
            ),
            LegacyDiscoveryRejection::InvalidParams => json_response(
                StatusCode::BAD_REQUEST,
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": message.get("id"),
                    "error": {
                        "code": -32602,
                        "message": "Invalid params",
                    },
                }),
            ),
            LegacyDiscoveryRejection::NotFound => {
                (StatusCode::NOT_FOUND, "legacy endpoint not found").into_response()
            }
            LegacyDiscoveryRejection::MethodNotAllowed => {
                (StatusCode::METHOD_NOT_ALLOWED, "legacy method not allowed").into_response()
            }
            LegacyDiscoveryRejection::Unauthorized => {
                (StatusCode::UNAUTHORIZED, "authentication required").into_response()
            }
            LegacyDiscoveryRejection::Forbidden => {
                (StatusCode::FORBIDDEN, "access forbidden").into_response()
            }
            LegacyDiscoveryRejection::UnauthorizedJson => json_response(
                StatusCode::UNAUTHORIZED,
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": null,
                    "error": {
                        "code": -32000,
                        "message": "Bad Request: No valid session ID provided",
                    },
                }),
            ),
            LegacyDiscoveryRejection::ForbiddenJson => json_response(
                StatusCode::FORBIDDEN,
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": null,
                    "error": {
                        "code": -32000,
                        "message": "Bad Request: Unsupported protocol version: 2026-07-28 \
                            (supported versions: 2025-11-25, 2025-06-18)",
                    },
                }),
            ),
            LegacyDiscoveryRejection::UnrelatedResponseId => json_response(
                StatusCode::BAD_REQUEST,
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 999,
                    "error": {
                        "code": -32602,
                        "message": "Invalid params",
                    },
                }),
            ),
            LegacyDiscoveryRejection::NotFoundWithUnrelatedResponseId => json_response(
                StatusCode::NOT_FOUND,
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 999,
                    "error": {
                        "code": -32601,
                        "message": "Method not found",
                    },
                }),
            ),
            LegacyDiscoveryRejection::ArbitraryBadRequest => json_response(
                StatusCode::BAD_REQUEST,
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": null,
                    "error": {
                        "code": -32000,
                        "message": "Bad Request: database unavailable",
                    },
                }),
            ),
            LegacyDiscoveryRejection::MixedFutureVersions => json_response(
                StatusCode::BAD_REQUEST,
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": null,
                    "error": {
                        "code": -32000,
                        "message": "Bad Request: Unsupported protocol version: 2026-07-28 \
                            (supported versions: 2025-06-18, 2027-01-01)",
                    },
                }),
            ),
            LegacyDiscoveryRejection::InternalServerErrorWithLegacyBody => json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": null,
                    "error": {
                        "code": -32000,
                        "message": "Bad Request: No valid session ID provided",
                    },
                }),
            ),
        },
        "initialize" => {
            assert_eq!(
                message
                    .get("params")
                    .and_then(|params| params.get("protocolVersion")),
                Some(&serde_json::json!("2025-06-18"))
            );
            let mut result = InitializeResult::new(ServerCapabilities::default());
            result.protocol_version = ProtocolVersion::V_2025_06_18;
            json_response(
                StatusCode::OK,
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": message.get("id"),
                    "result": result,
                }),
            )
        }
        "notifications/initialized" => StatusCode::ACCEPTED.into_response(),
        "tools/list" => json_response(
            StatusCode::OK,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": message.get("id"),
                "result": { "tools": [] },
            }),
        ),
        _ => (StatusCode::BAD_REQUEST, "unexpected request").into_response(),
    }
}

fn json_response(status: StatusCode, value: serde_json::Value) -> Response {
    (
        status,
        [(header::CONTENT_TYPE, "application/json")],
        value.to_string(),
    )
        .into_response()
}

async fn assert_http_legacy_fallback(rejection: LegacyDiscoveryRejection) {
    let methods = Arc::new(Mutex::new(Vec::new()));
    let router = axum::Router::new()
        .route("/mcp", post(legacy_prevalidation_handler))
        .with_state(LegacyPrevalidationState {
            rejection,
            methods: methods.clone(),
        });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener should bind");
    let address = listener.local_addr().expect("listener address");
    let cancellation = CancellationToken::new();
    let server = tokio::spawn({
        let cancellation = cancellation.clone();
        async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(cancellation.cancelled_owned())
                .await
                .expect("serve legacy HTTP endpoint");
        }
    });

    let transport = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(format!("http://{address}/mcp")),
    );
    let client = ClientInfo::default()
        .serve_with_lifecycle(
            transport,
            ClientLifecycleMode::Auto {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
                legacy_version: Some(ProtocolVersion::V_2025_06_18),
            },
        )
        .await
        .expect("Auto mode should recognize the deployed legacy HTTP response");
    client.list_tools(None).await.expect("list legacy tools");
    client.cancel().await.expect("cancel client");

    assert_eq!(
        *methods.lock().expect("methods lock"),
        [
            "server/discover",
            "initialize",
            "notifications/initialized",
            "tools/list",
        ]
    );
    cancellation.cancel();
    server.await.expect("server task");
}

#[tokio::test]
async fn auto_http_client_falls_back_for_recognized_legacy_rejections() {
    for rejection in [
        LegacyDiscoveryRejection::UnsupportedProtocol,
        LegacyDiscoveryRejection::UnsupportedProtocolWithoutVersionList,
        LegacyDiscoveryRejection::MissingSession,
        LegacyDiscoveryRejection::InvalidRequest,
        LegacyDiscoveryRejection::InvalidParams,
        LegacyDiscoveryRejection::NotFound,
        LegacyDiscoveryRejection::MethodNotAllowed,
    ] {
        assert_http_legacy_fallback(rejection).await;
    }
}

async fn assert_http_rejection_does_not_downgrade(rejection: LegacyDiscoveryRejection) {
    let methods = Arc::new(Mutex::new(Vec::new()));
    let router = axum::Router::new()
        .route("/mcp", post(legacy_prevalidation_handler))
        .with_state(LegacyPrevalidationState {
            rejection,
            methods: methods.clone(),
        });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener should bind");
    let address = listener.local_addr().expect("listener address");
    let cancellation = CancellationToken::new();
    let server = tokio::spawn({
        let cancellation = cancellation.clone();
        async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(cancellation.cancelled_owned())
                .await
                .expect("serve auth-rejecting HTTP endpoint");
        }
    });

    let transport = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(format!("http://{address}/mcp")),
    );
    let result = ClientInfo::default()
        .serve_with_lifecycle(
            transport,
            ClientLifecycleMode::Auto {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
                legacy_version: Some(ProtocolVersion::V_2025_06_18),
            },
        )
        .await;
    assert!(
        result.is_err(),
        "authentication failures must not downgrade"
    );
    assert_eq!(*methods.lock().expect("methods lock"), ["server/discover"]);
    cancellation.cancel();
    server.await.expect("server task");
}

#[tokio::test]
async fn auto_http_client_rejects_unsafe_downgrade_signals() {
    for rejection in [
        LegacyDiscoveryRejection::Unauthorized,
        LegacyDiscoveryRejection::Forbidden,
        LegacyDiscoveryRejection::UnauthorizedJson,
        LegacyDiscoveryRejection::ForbiddenJson,
        LegacyDiscoveryRejection::UnrelatedResponseId,
        LegacyDiscoveryRejection::NotFoundWithUnrelatedResponseId,
        LegacyDiscoveryRejection::ArbitraryBadRequest,
        LegacyDiscoveryRejection::MixedFutureVersions,
        LegacyDiscoveryRejection::InternalServerErrorWithLegacyBody,
    ] {
        assert_http_rejection_does_not_downgrade(rejection).await;
    }
}

#[derive(Debug, thiserror::Error)]
#[error("mock HTTP client error")]
struct MockHttpClientError;

#[derive(Clone, Default)]
struct TypedProbeFailureClient {
    methods: Arc<Mutex<Vec<String>>>,
    retry_modern: bool,
}

impl StreamableHttpClient for TypedProbeFailureClient {
    type Error = MockHttpClientError;

    async fn post_message(
        &self,
        _uri: Arc<str>,
        message: ClientJsonRpcMessage,
        _session_id: Option<Arc<str>>,
        _auth_header: Option<String>,
        _custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>> {
        let method = match &message {
            ClientJsonRpcMessage::Request(request) => match &request.request {
                ClientRequest::DiscoverRequest(_) => "server/discover",
                ClientRequest::InitializeRequest(_) => "initialize",
                other => panic!("unexpected request: {other:?}"),
            },
            ClientJsonRpcMessage::Notification(_) => "notifications/initialized",
            other => panic!("unexpected client message: {other:?}"),
        };
        let discover_attempt = {
            let mut methods = self.methods.lock().expect("methods lock");
            methods.push(method.to_owned());
            methods
                .iter()
                .filter(|method| method.as_str() == "server/discover")
                .count()
        };

        match message {
            ClientJsonRpcMessage::Request(request)
                if matches!(&request.request, ClientRequest::DiscoverRequest(_)) =>
            {
                if self.retry_modern && discover_attempt == 1 {
                    Err(StreamableHttpError::UnexpectedHttpStatus(
                        HttpStatusError::new(
                            400,
                            serde_json::json!({
                                "jsonrpc": "2.0",
                                "id": request.id,
                                "error": {
                                    "code": -32022,
                                    "message": "Unsupported protocol version",
                                    "data": {
                                        "supported": ["2026-07-28"],
                                        "requested": "2026-07-28",
                                    },
                                },
                            })
                            .to_string(),
                        ),
                    ))
                } else if self.retry_modern {
                    Ok(StreamableHttpPostResponse::Json(
                        ServerJsonRpcMessage::response(
                            ServerResult::DiscoverResult(DiscoverResult::new(
                                vec![ProtocolVersion::V_2026_07_28],
                                ServerCapabilities::default(),
                                Implementation::new("modern-server", "1.0.0"),
                            )),
                            request.id,
                        ),
                        None,
                    ))
                } else {
                    Err(StreamableHttpError::UnexpectedHttpStatus(
                        HttpStatusError::new(
                            400,
                            r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32000,"message":"Bad Request: Unsupported protocol version: 2026-07-28"}}"#,
                        ),
                    ))
                }
            }
            ClientJsonRpcMessage::Request(request)
                if matches!(&request.request, ClientRequest::InitializeRequest(_)) =>
            {
                Ok(StreamableHttpPostResponse::Json(
                    ServerJsonRpcMessage::response(
                        ServerResult::InitializeResult(
                            InitializeResult::new(ServerCapabilities::default())
                                .with_protocol_version(ProtocolVersion::V_2025_06_18),
                        ),
                        request.id,
                    ),
                    Some("legacy-session".to_owned()),
                ))
            }
            ClientJsonRpcMessage::Notification(_) => Ok(StreamableHttpPostResponse::Accepted),
            other => panic!("unexpected client message: {other:?}"),
        }
    }

    async fn delete_session(
        &self,
        _uri: Arc<str>,
        _session_id: Arc<str>,
        _auth_header: Option<String>,
        _custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<(), StreamableHttpError<Self::Error>> {
        Ok(())
    }

    async fn get_stream(
        &self,
        _uri: Arc<str>,
        _session_id: Option<Arc<str>>,
        _last_event_id: Option<String>,
        _auth_header: Option<String>,
        _custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<
        futures::stream::BoxStream<'static, Result<sse_stream::Sse, sse_stream::Error>>,
        StreamableHttpError<Self::Error>,
    > {
        Ok(stream::pending().boxed())
    }
}

#[tokio::test]
async fn auto_mode_consumes_typed_probe_failures_from_custom_http_clients() {
    let http_client = TypedProbeFailureClient::default();
    let methods = http_client.methods.clone();
    let transport = StreamableHttpClientTransport::with_client(
        http_client,
        StreamableHttpClientTransportConfig::with_uri("http://custom.invalid/mcp"),
    );

    let client = ClientInfo::default()
        .serve_with_lifecycle(
            transport,
            ClientLifecycleMode::Auto {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
                legacy_version: Some(ProtocolVersion::V_2025_06_18),
            },
        )
        .await
        .expect("Auto mode should consume the typed custom-client probe failure");
    client.cancel().await.expect("cancel client");

    assert_eq!(
        *methods.lock().expect("methods lock"),
        ["server/discover", "initialize", "notifications/initialized"]
    );
}

#[tokio::test]
async fn discover_mode_consumes_json_rpc_errors_from_typed_http_400_responses() {
    let http_client = TypedProbeFailureClient {
        retry_modern: true,
        ..Default::default()
    };
    let methods = http_client.methods.clone();
    let transport = StreamableHttpClientTransport::with_client(
        http_client,
        StreamableHttpClientTransportConfig::with_uri("http://custom.invalid/mcp"),
    );

    let client = ClientInfo::default()
        .serve_with_lifecycle(
            transport,
            ClientLifecycleMode::Discover {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            },
        )
        .await
        .expect("Discover mode should retry a modern version from a typed HTTP 400 response");
    client.cancel().await.expect("cancel client");

    assert_eq!(
        *methods.lock().expect("methods lock"),
        ["server/discover", "server/discover"]
    );
}
