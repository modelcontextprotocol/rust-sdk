#![cfg(all(feature = "client", feature = "server", not(feature = "local")))]

use rmcp::{
    ClientHandler, ClientLifecycleMode, ClientServiceExt, ServerHandler, ServiceExt,
    model::{
        ClientJsonRpcMessage, ClientRequest, DiscoverResult, ErrorCode, ErrorData, GetMeta,
        Implementation, InitializeResult, ProtocolVersion, RequestId, ServerCapabilities,
        ServerJsonRpcMessage, ServerResult,
    },
    service::PeerRequestOptions,
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
                ServerResult::DiscoverResult(
                    DiscoverResult::new(
                        vec![ProtocolVersion::V_2026_07_28],
                        ServerCapabilities::default(),
                    )
                    .with_server_info(Implementation::new("discover-server", "1.0.0")),
                ),
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
#[allow(deprecated)]
async fn discover_startup_accepts_missing_optional_server_info() {
    let (server_transport, client_transport) = tokio::io::duplex(4096);
    let mut server = IntoTransport::<rmcp::RoleServer, _, _>::into_transport(server_transport);
    let (rejection_observed_tx, rejection_observed_rx) = tokio::sync::oneshot::channel();
    let server_task = tokio::spawn(async move {
        let ClientJsonRpcMessage::Request(discover_request) =
            server.receive().await.expect("expected discover request")
        else {
            panic!("expected discover request");
        };
        let mut result = DiscoverResult::new(
            vec![ProtocolVersion::V_2026_07_28],
            ServerCapabilities::builder().enable_tools().build(),
        );
        result.instructions = Some("discovery instructions".into());
        result.meta = Some(rmcp::model::MetaObject::new());
        result
            .meta
            .as_mut()
            .expect("metadata")
            .0
            .insert("example.test/key".into(), serde_json::json!(7));
        server
            .send(ServerJsonRpcMessage::response(
                ServerResult::DiscoverResult(result),
                discover_request.id,
            ))
            .await
            .expect("send discover response");

        server
            .send(ServerJsonRpcMessage::request(
                rmcp::model::ServerRequest::CreateMessageRequest(
                    rmcp::model::CreateMessageRequest::new(
                        rmcp::model::CreateMessageRequestParams::new(
                            vec![rmcp::model::SamplingMessage::user_text("unsolicited")],
                            16,
                        ),
                    ),
                ),
                RequestId::Number(99),
            ))
            .await
            .expect("send unsolicited server request");
        let Some(ClientJsonRpcMessage::Error(error)) = server.receive().await else {
            panic!("expected unsolicited server request to be rejected");
        };
        assert_eq!(error.error.code, ErrorCode::INVALID_PARAMS);
        rejection_observed_tx
            .send(())
            .expect("signal observed rejection");

        let ClientJsonRpcMessage::Request(request) =
            server.receive().await.expect("expected normal request")
        else {
            panic!("expected normal request");
        };
        assert_eq!(
            request.request.get_meta().protocol_version(),
            Some(ProtocolVersion::V_2026_07_28)
        );
        server
            .send(ServerJsonRpcMessage::response(
                ServerResult::ListToolsResult(Default::default()),
                request.id,
            ))
            .await
            .expect("send list tools response");
    });

    let client = DiscoverClient
        .serve_with_lifecycle(
            client_transport,
            ClientLifecycleMode::Discover {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            },
        )
        .await
        .expect("missing optional server info should not fail discovery");
    let peer_info = client.peer_info().expect("peer info should be retained");
    assert_eq!(peer_info.protocol_version, ProtocolVersion::V_2026_07_28);
    assert!(peer_info.capabilities.tools.is_some());
    assert_eq!(peer_info.server_info, None);
    assert_eq!(
        peer_info.instructions.as_deref(),
        Some("discovery instructions")
    );
    assert_eq!(
        peer_info
            .meta
            .as_ref()
            .and_then(|meta| meta.0.get("example.test/key")),
        Some(&serde_json::json!(7))
    );
    rejection_observed_rx
        .await
        .expect("server should observe association rejection");
    client.list_tools(None).await.expect("list tools");
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
                ServerResult::DiscoverResult(
                    DiscoverResult::new(
                        vec![ProtocolVersion::V_2026_07_28],
                        ServerCapabilities::default(),
                    )
                    .with_server_info(Implementation::new("discover-server", "1.0.0")),
                ),
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

/// Drives an `Auto` client through a single `server/discover` probe and asserts
/// the legacy fallback decision against the response the server sends back.
///
/// When `expect_fallback` is set, the server also accepts the subsequent
/// `initialize` request and the client is expected to connect. Otherwise the
/// client must surface the discover error without sending `initialize`, and the
/// server's next receive must not be an initialize request.
async fn run_auto_discover_response_scenario(error: ErrorData, expect_fallback: bool) {
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
            .send(ServerJsonRpcMessage::error(error, Some(discover.id)))
            .await
            .expect("send discover error response");

        if expect_fallback {
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
                    ServerResult::InitializeResult(InitializeResult::new(
                        ServerCapabilities::default(),
                    )),
                    initialize.id,
                ))
                .await
                .expect("send initialize response");
            assert!(matches!(
                server.receive().await,
                Some(ClientJsonRpcMessage::Notification(_))
            ));
        } else {
            // The client must surface the error without falling back, so no
            // initialize request should follow. The transport closes when the
            // failed client is dropped.
            if let Some(ClientJsonRpcMessage::Request(request)) = server.receive().await {
                panic!(
                    "client fell back to {:?} but should have surfaced the modern error",
                    request.request
                );
            }
        }
    });

    let client_result = DiscoverClient
        .serve_with_lifecycle(
            client_transport,
            ClientLifecycleMode::Auto {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
                legacy_version: Some(ProtocolVersion::V_2025_11_25),
            },
        )
        .await;

    if expect_fallback {
        let client = client_result.expect("auto client should fall back to initialize");
        client.cancel().await.expect("cancel client");
    } else {
        assert!(
            client_result.is_err(),
            "modern error should surface without legacy fallback"
        );
    }
    server_task.await.expect("server task");
}

#[tokio::test]
async fn auto_startup_falls_back_after_discover_invalid_request() {
    // Legacy servers commonly reject an unknown pre-initialize request with
    // `-32600` (e.g. a session middleware that requires `initialize` first).
    run_auto_discover_response_scenario(
        ErrorData::new(ErrorCode::INVALID_REQUEST, "Bad Request", None),
        true,
    )
    .await;
}

#[tokio::test]
async fn auto_startup_falls_back_after_discover_invalid_params() {
    // `-32602` is explicitly called out by the specification as an
    // implementation-defined response legacy servers use for unknown requests.
    run_auto_discover_response_scenario(
        ErrorData::new(ErrorCode::INVALID_PARAMS, "Invalid params", None),
        true,
    )
    .await;
}

#[tokio::test]
async fn auto_startup_does_not_fall_back_for_missing_required_capability() {
    // A `MISSING_REQUIRED_CLIENT_CAPABILITY` response identifies a modern
    // server; falling back to `initialize` would not address it.
    run_auto_discover_response_scenario(
        ErrorData::new(
            ErrorCode::MISSING_REQUIRED_CLIENT_CAPABILITY,
            "Missing required client capability",
            None,
        ),
        false,
    )
    .await;
}

#[tokio::test]
async fn auto_startup_does_not_fall_back_for_header_mismatch() {
    // A `HEADER_MISMATCH` response identifies a modern server performing
    // header validation; falling back to `initialize` would not address it.
    run_auto_discover_response_scenario(
        ErrorData::new(ErrorCode::HEADER_MISMATCH, "Header mismatch", None),
        false,
    )
    .await;
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
                ServerResult::DiscoverResult(
                    DiscoverResult::new(
                        vec![ProtocolVersion::V_2026_07_28],
                        ServerCapabilities::default(),
                    )
                    .with_server_info(Implementation::new("discover-server", "1.0.0")),
                ),
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
                ServerResult::DiscoverResult(
                    DiscoverResult::new(
                        vec![ProtocolVersion::V_2026_07_28],
                        ServerCapabilities::default(),
                    )
                    .with_server_info(Implementation::new("discover-server", "1.0.0")),
                ),
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
