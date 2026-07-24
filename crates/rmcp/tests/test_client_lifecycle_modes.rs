#![cfg(all(feature = "client", feature = "server", not(feature = "local")))]

use rmcp::{
    ClientHandler, ClientLifecycleMode, ClientServiceExt, ServerHandler, ServiceExt,
    model::{
        ClientJsonRpcMessage, ClientRequest, DiscoverResult, ErrorCode, ErrorData, GetMeta,
        Implementation, InitializeResult, ProtocolVersion, RequestId, ServerCapabilities,
        ServerJsonRpcMessage, ServerResult,
    },
    service::{ClientInitializeError, PeerRequestOptions},
    transport::{IntoTransport, Transport},
};

#[derive(Clone, Default)]
struct DiscoverClient;

impl ClientHandler for DiscoverClient {}

#[derive(Clone, Default)]
struct StatelessServer;

impl ServerHandler for StatelessServer {}

#[tokio::test]
async fn discover_startup_accepts_stringified_numeric_response_id() {
    let (server_transport, client_transport) = tokio::io::duplex(4096);
    let mut server = IntoTransport::<rmcp::RoleServer, _, _>::into_transport(server_transport);
    let server_task = tokio::spawn(async move {
        let ClientJsonRpcMessage::Request(request) =
            server.receive().await.expect("expected discover request")
        else {
            panic!("expected discover request");
        };
        let RequestId::Number(response_id) = request.id else {
            panic!("expected a numeric request ID");
        };
        server
            .send(ServerJsonRpcMessage::response(
                ServerResult::DiscoverResult(DiscoverResult::new(
                    vec![ProtocolVersion::V_2026_07_28],
                    ServerCapabilities::default(),
                    Implementation::new("discover-server", "1.0.0"),
                )),
                RequestId::String(response_id.to_string().into()),
            ))
            .await
            .expect("send discover response");
    });

    let client = DiscoverClient
        .serve_with_lifecycle(
            client_transport,
            ClientLifecycleMode::Discover {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            },
        )
        .await
        .expect("client should accept stringified discover response ID");
    client.cancel().await.expect("cancel client");
    server_task.await.expect("server task");
}

#[tokio::test]
async fn high_level_server_accepts_discover_startup_without_initialize() {
    let (server_transport, client_transport) = tokio::io::duplex(4096);
    let server_task = tokio::spawn(async move {
        StatelessServer
            .serve(server_transport)
            .await
            .expect("server should accept discover")
    });

    let client = DiscoverClient
        .serve_with_lifecycle(
            client_transport,
            ClientLifecycleMode::Discover {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            },
        )
        .await
        .expect("discover client should start");
    client.list_tools(None).await.expect("list tools");
    client.cancel().await.expect("cancel client");
    let server = server_task.await.expect("server task");
    server.cancel().await.expect("cancel server");
}

#[tokio::test]
async fn discover_startup_omits_initialize() {
    let (server_transport, client_transport) = tokio::io::duplex(4096);
    let mut server = IntoTransport::<rmcp::RoleServer, _, _>::into_transport(server_transport);
    let server_task = tokio::spawn(async move {
        let ClientJsonRpcMessage::Request(request) =
            server.receive().await.expect("expected discover request")
        else {
            panic!("expected request");
        };
        assert!(matches!(request.request, ClientRequest::DiscoverRequest(_)));
        let meta = request.request.get_meta();
        assert_eq!(meta.protocol_version(), Some(ProtocolVersion::V_2026_07_28));
        assert!(meta.client_info().is_some());
        assert!(meta.client_capabilities().is_some());

        server
            .send(ServerJsonRpcMessage::response(
                ServerResult::DiscoverResult(DiscoverResult::new(
                    vec![ProtocolVersion::V_2026_07_28],
                    ServerCapabilities::default(),
                    Implementation::new("discover-server", "1.0.0"),
                )),
                request.id,
            ))
            .await
            .expect("send discover response");

        let ClientJsonRpcMessage::Request(request) =
            server.receive().await.expect("expected normal request")
        else {
            panic!("expected request");
        };
        assert!(!matches!(
            request.request,
            ClientRequest::InitializeRequest(_)
        ));
        let meta = request.request.get_meta();
        assert_eq!(meta.protocol_version(), Some(ProtocolVersion::V_2025_11_25));
        assert!(meta.client_info().is_some());
        assert!(meta.client_capabilities().is_some());
        assert_eq!(
            meta.get("example.test/extension"),
            Some(&serde_json::json!(7))
        );
        server
            .send(ServerJsonRpcMessage::response(
                ServerResult::ListToolsResult(Default::default()),
                request.id,
            ))
            .await
            .expect("send tools response");
    });

    let client = DiscoverClient
        .serve_with_lifecycle(
            client_transport,
            ClientLifecycleMode::Discover {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            },
        )
        .await
        .expect("discover client should start");
    let mut caller_meta = rmcp::model::RequestMetaObject::new();
    caller_meta.insert("example.test/extension".into(), serde_json::json!(7));
    caller_meta.set_protocol_version(ProtocolVersion::V_2025_11_25);
    client
        .send_request_with_option(
            ClientRequest::ListToolsRequest(rmcp::model::ListToolsRequest {
                method: Default::default(),
                params: None,
                extensions: Default::default(),
            }),
            PeerRequestOptions::default().with_meta(caller_meta),
        )
        .await
        .expect("send list tools")
        .await_response()
        .await
        .expect("list tools response");
    client.cancel().await.expect("cancel client");
    server_task.await.expect("server task");
}

#[tokio::test]
async fn auto_startup_falls_back_after_discover_method_not_found() {
    let (server_transport, client_transport) = tokio::io::duplex(4096);
    let mut server = IntoTransport::<rmcp::RoleServer, _, _>::into_transport(server_transport);
    let server_task = tokio::spawn(async move {
        let ClientJsonRpcMessage::Request(discover) =
            server.receive().await.expect("expected discover request")
        else {
            panic!("expected request");
        };
        assert!(matches!(
            discover.request,
            ClientRequest::DiscoverRequest(_)
        ));
        server
            .send(ServerJsonRpcMessage::error(
                ErrorData::new(ErrorCode::METHOD_NOT_FOUND, "Method not found", None),
                Some(discover.id),
            ))
            .await
            .expect("send method-not-found");

        let ClientJsonRpcMessage::Request(initialize) =
            server.receive().await.expect("expected initialize request")
        else {
            panic!("expected request");
        };
        assert!(matches!(
            initialize.request,
            ClientRequest::InitializeRequest(_)
        ));
        server
            .send(ServerJsonRpcMessage::response(
                ServerResult::InitializeResult(
                    InitializeResult::new(ServerCapabilities::default()),
                ),
                initialize.id,
            ))
            .await
            .expect("send initialize response");
        assert!(matches!(
            server.receive().await,
            Some(ClientJsonRpcMessage::Notification(_))
        ));
    });

    let client = DiscoverClient
        .serve_with_lifecycle(
            client_transport,
            ClientLifecycleMode::Auto {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
                legacy_version: Some(ProtocolVersion::V_2025_11_25),
            },
        )
        .await
        .expect("auto client should fall back");
    client.cancel().await.expect("cancel client");
    server_task.await.expect("server task");
}

async fn assert_auto_startup_falls_back(
    rejection: impl FnOnce(RequestId) -> ServerJsonRpcMessage + Send + 'static,
) {
    let (server_transport, client_transport) = tokio::io::duplex(4096);
    let mut server = IntoTransport::<rmcp::RoleServer, _, _>::into_transport(server_transport);
    let server_task = tokio::spawn(async move {
        let ClientJsonRpcMessage::Request(discover) =
            server.receive().await.expect("expected discover request")
        else {
            panic!("expected discover request");
        };
        assert!(matches!(
            discover.request,
            ClientRequest::DiscoverRequest(_)
        ));
        server
            .send(rejection(discover.id))
            .await
            .expect("send legacy discovery rejection");

        let ClientJsonRpcMessage::Request(initialize) = server
            .receive()
            .await
            .expect("expected fallback initialize request")
        else {
            panic!("expected fallback initialize request");
        };
        let ClientRequest::InitializeRequest(request) = initialize.request else {
            panic!("expected initialize request");
        };
        assert_eq!(
            request.params.protocol_version,
            ProtocolVersion::V_2025_06_18
        );
        server
            .send(ServerJsonRpcMessage::response(
                ServerResult::InitializeResult(
                    InitializeResult::new(ServerCapabilities::default()),
                ),
                initialize.id,
            ))
            .await
            .expect("send initialize response");
        assert!(matches!(
            server.receive().await,
            Some(ClientJsonRpcMessage::Notification(_))
        ));
    });

    let client = DiscoverClient
        .serve_with_lifecycle(
            client_transport,
            ClientLifecycleMode::Auto {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
                legacy_version: Some(ProtocolVersion::V_2025_06_18),
            },
        )
        .await
        .expect("Auto mode should fall back for a recognized legacy-only rejection");
    client.cancel().await.expect("cancel client");
    server_task.await.expect("server task");
}

#[tokio::test]
async fn auto_startup_falls_back_when_server_only_supports_historical_versions() {
    assert_auto_startup_falls_back(|id| {
        ServerJsonRpcMessage::error(
            ErrorData::unsupported_protocol_version(
                ProtocolVersion::V_2026_07_28,
                &[ProtocolVersion::V_2025_11_25, ProtocolVersion::V_2025_06_18],
            ),
            Some(id),
        )
    })
    .await;
}

#[tokio::test]
async fn auto_startup_falls_back_when_discovery_only_advertises_historical_versions() {
    assert_auto_startup_falls_back(|id| {
        ServerJsonRpcMessage::response(
            ServerResult::DiscoverResult(DiscoverResult::new(
                vec![ProtocolVersion::V_2025_06_18],
                ServerCapabilities::default(),
                Implementation::new("legacy-server", "1.0.0"),
            )),
            id,
        )
    })
    .await;
}

#[tokio::test]
async fn auto_startup_falls_back_for_uncorrelated_legacy_protocol_prevalidation() {
    assert_auto_startup_falls_back(|_| {
        ServerJsonRpcMessage::error(
            ErrorData::new(
                ErrorCode(-32000),
                "Bad Request: Unsupported protocol version: 2026-07-28 \
                 (supported versions: 2025-11-25, 2025-06-18, 2025-03-26, \
                 2024-11-05, 2024-10-07)",
                None,
            ),
            None,
        )
    })
    .await;
}

#[tokio::test]
async fn auto_startup_falls_back_for_uncorrelated_missing_session_prevalidation() {
    assert_auto_startup_falls_back(|_| {
        ServerJsonRpcMessage::error(
            ErrorData::new(
                ErrorCode(-32000),
                "Bad Request: No valid session ID provided",
                None,
            ),
            None,
        )
    })
    .await;
}

async fn assert_auto_startup_does_not_fall_back(
    rejection: impl FnOnce(RequestId) -> ServerJsonRpcMessage + Send + 'static,
) -> ClientInitializeError {
    let (server_transport, client_transport) = tokio::io::duplex(4096);
    let mut server = IntoTransport::<rmcp::RoleServer, _, _>::into_transport(server_transport);
    let server_task = tokio::spawn(async move {
        let ClientJsonRpcMessage::Request(discover) =
            server.receive().await.expect("expected discover request")
        else {
            panic!("expected discover request");
        };
        server
            .send(rejection(discover.id))
            .await
            .expect("send discovery rejection");
        assert!(
            server.receive().await.is_none(),
            "unsafe discovery rejection must not trigger initialize"
        );
    });

    let error = match DiscoverClient
        .serve_with_lifecycle(
            client_transport,
            ClientLifecycleMode::Auto {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
                legacy_version: Some(ProtocolVersion::V_2025_06_18),
            },
        )
        .await
    {
        Ok(_) => panic!("unsafe discovery rejection must not trigger fallback"),
        Err(error) => error,
    };
    server_task.await.expect("server task");
    error
}

#[tokio::test]
async fn auto_startup_does_not_fall_back_for_unrelated_error_response_id() {
    let error = assert_auto_startup_does_not_fall_back(|_| {
        ServerJsonRpcMessage::error(
            ErrorData::new(ErrorCode::METHOD_NOT_FOUND, "Method not found", None),
            Some(RequestId::Number(999)),
        )
    })
    .await;
    assert!(matches!(
        error,
        ClientInitializeError::ConflictInitResponseId(_, _)
    ));
}

#[tokio::test]
async fn auto_startup_does_not_fall_back_for_uncorrelated_method_not_found() {
    let error = assert_auto_startup_does_not_fall_back(|_| {
        ServerJsonRpcMessage::error(
            ErrorData::new(ErrorCode::METHOD_NOT_FOUND, "Method not found", None),
            None,
        )
    })
    .await;
    assert!(matches!(error, ClientInitializeError::JsonRpcError(_)));
}

#[tokio::test]
async fn auto_startup_does_not_fall_back_for_arbitrary_uncorrelated_errors() {
    let error = assert_auto_startup_does_not_fall_back(|_| {
        ServerJsonRpcMessage::error(
            ErrorData::new(ErrorCode(-32000), "Bad Request: database unavailable", None),
            None,
        )
    })
    .await;
    assert!(matches!(error, ClientInitializeError::JsonRpcError(_)));
}

#[tokio::test]
async fn auto_startup_does_not_fall_back_for_unknown_or_future_protocol_versions() {
    let error = assert_auto_startup_does_not_fall_back(|_| {
        ServerJsonRpcMessage::error(
            ErrorData::new(
                ErrorCode(-32000),
                "Bad Request: Unsupported protocol version: 2026-07-28 \
                 (supported versions: 2025-06-18, 2027-01-01)",
                None,
            ),
            None,
        )
    })
    .await;
    assert!(matches!(error, ClientInitializeError::JsonRpcError(_)));
}

#[tokio::test]
async fn auto_startup_does_not_fall_back_for_unknown_protocol_version_tokens() {
    let error = assert_auto_startup_does_not_fall_back(|_| {
        ServerJsonRpcMessage::error(
            ErrorData::new(
                ErrorCode(-32000),
                "Bad Request: Unsupported protocol version: 2026-07-28 \
                 (supported versions: 2025-06-18, next-draft)",
                None,
            ),
            None,
        )
    })
    .await;
    assert!(matches!(error, ClientInitializeError::JsonRpcError(_)));
}

#[tokio::test]
async fn auto_startup_does_not_fall_back_for_contradictory_supported_versions() {
    let error = assert_auto_startup_does_not_fall_back(|_| {
        ServerJsonRpcMessage::error(
            ErrorData::new(
                ErrorCode(-32000),
                "Bad Request: Unsupported protocol version: 2026-07-28 \
                 (supported versions: 2025-06-18, 2027-01-01)",
                Some(serde_json::json!({ "supported": ["2025-06-18"] })),
            ),
            None,
        )
    })
    .await;
    assert!(matches!(error, ClientInitializeError::JsonRpcError(_)));
}

#[tokio::test]
async fn auto_startup_does_not_fall_back_for_uncorrelated_protocol_version_error() {
    let error = assert_auto_startup_does_not_fall_back(|_| {
        ServerJsonRpcMessage::error(
            ErrorData::unsupported_protocol_version(
                ProtocolVersion::V_2026_07_28,
                &[ProtocolVersion::V_2025_06_18],
            ),
            None,
        )
    })
    .await;
    assert!(matches!(error, ClientInitializeError::JsonRpcError(_)));
}

#[tokio::test]
async fn discover_startup_retries_a_mutually_supported_version() {
    let unsupported: ProtocolVersion =
        serde_json::from_value(serde_json::json!("2099-01-01")).unwrap();
    let (server_transport, client_transport) = tokio::io::duplex(4096);
    let mut server = IntoTransport::<rmcp::RoleServer, _, _>::into_transport(server_transport);
    let server_task = tokio::spawn(async move {
        let ClientJsonRpcMessage::Request(first) =
            server.receive().await.expect("expected first discover")
        else {
            panic!("expected request");
        };
        assert_eq!(
            first.request.get_meta().protocol_version(),
            Some(unsupported.clone())
        );
        server
            .send(ServerJsonRpcMessage::error(
                ErrorData::unsupported_protocol_version(
                    unsupported,
                    &[ProtocolVersion::V_2026_07_28],
                ),
                Some(first.id),
            ))
            .await
            .expect("send unsupported error");

        let ClientJsonRpcMessage::Request(second) =
            server.receive().await.expect("expected retry discover")
        else {
            panic!("expected request");
        };
        assert_eq!(
            second.request.get_meta().protocol_version(),
            Some(ProtocolVersion::V_2026_07_28)
        );
        server
            .send(ServerJsonRpcMessage::response(
                ServerResult::DiscoverResult(DiscoverResult::new(
                    vec![ProtocolVersion::V_2026_07_28],
                    ServerCapabilities::default(),
                    Implementation::new("discover-server", "1.0.0"),
                )),
                second.id,
            ))
            .await
            .expect("send discover response");
    });

    let client = DiscoverClient
        .serve_with_lifecycle(
            client_transport,
            ClientLifecycleMode::Discover {
                preferred_versions: vec![
                    serde_json::from_value(serde_json::json!("2099-01-01")).unwrap(),
                    ProtocolVersion::V_2026_07_28,
                ],
            },
        )
        .await
        .expect("discover client should retry");
    client.cancel().await.expect("cancel client");
    server_task.await.expect("server task");
}

#[tokio::test]
async fn discover_startup_retries_current_version_once_when_server_reports_it_supported() {
    let (server_transport, client_transport) = tokio::io::duplex(4096);
    let mut server = IntoTransport::<rmcp::RoleServer, _, _>::into_transport(server_transport);
    let server_task = tokio::spawn(async move {
        let ClientJsonRpcMessage::Request(first) =
            server.receive().await.expect("expected first discover")
        else {
            panic!("expected request");
        };
        server
            .send(ServerJsonRpcMessage::error(
                ErrorData::unsupported_protocol_version(
                    ProtocolVersion::V_2026_07_28,
                    &[ProtocolVersion::V_2026_07_28],
                ),
                Some(first.id),
            ))
            .await
            .expect("send unsupported error");

        let ClientJsonRpcMessage::Request(second) =
            server.receive().await.expect("expected retry discover")
        else {
            panic!("expected request");
        };
        assert_eq!(
            second.request.get_meta().protocol_version(),
            Some(ProtocolVersion::V_2026_07_28)
        );
        server
            .send(ServerJsonRpcMessage::response(
                ServerResult::DiscoverResult(DiscoverResult::new(
                    vec![ProtocolVersion::V_2026_07_28],
                    ServerCapabilities::default(),
                    Implementation::new("discover-server", "1.0.0"),
                )),
                second.id,
            ))
            .await
            .expect("send discover response");
    });

    let client = DiscoverClient
        .serve_with_lifecycle(
            client_transport,
            ClientLifecycleMode::Discover {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            },
        )
        .await
        .expect("discover client should retry once");
    client.cancel().await.expect("cancel client");
    server_task.await.expect("server task");
}
