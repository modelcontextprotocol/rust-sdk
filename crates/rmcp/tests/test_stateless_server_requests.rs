#![cfg(all(feature = "server", not(feature = "local")))]

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use rmcp::{
    ServerHandler, ServiceExt,
    model::{
        ClientCapabilities, ClientJsonRpcMessage, ClientRequest, DiscoverRequest,
        DiscoverRequestParams, ErrorCode, ErrorData, Implementation, ListToolsRequest,
        ListToolsResult, NumberOrString, PaginatedRequestParams, ProgressNotificationParam,
        ProgressToken, ProtocolVersion, RequestId, RequestMetaObject, ServerJsonRpcMessage,
        ServerNotification,
    },
    service::{MaybeSendFuture, RequestContext, RoleServer},
    transport::{IntoTransport, Transport},
};

#[derive(Clone, Default)]
struct StatelessServer;

impl ServerHandler for StatelessServer {}

fn complete_meta() -> RequestMetaObject {
    complete_meta_for("stateless-client")
}

fn complete_meta_for(client_name: &str) -> RequestMetaObject {
    let mut meta = RequestMetaObject::new();
    meta.set_protocol_version(ProtocolVersion::V_2026_07_28);
    meta.set_client_info(Implementation::new(client_name, "1.0.0"));
    meta.set_client_capabilities(ClientCapabilities::default());
    meta
}

fn list_tools_request(meta: RequestMetaObject) -> ClientJsonRpcMessage {
    let mut request = ListToolsRequest {
        method: Default::default(),
        params: None,
        extensions: Default::default(),
    };
    request.extensions.insert(meta);
    ClientJsonRpcMessage::request(
        ClientRequest::ListToolsRequest(request),
        RequestId::Number(1),
    )
}

#[tokio::test]
async fn stateless_server_rejects_missing_metadata_on_every_request() {
    let (server_transport, client_transport) = tokio::io::duplex(4096);
    let server_task = tokio::spawn(async move {
        StatelessServer
            .serve(server_transport)
            .await
            .expect("server should start")
    });
    let mut client = IntoTransport::<rmcp::RoleClient, _, _>::into_transport(client_transport);

    let mut discover = DiscoverRequest::new(DiscoverRequestParams {});
    discover.extensions.insert(complete_meta());
    client
        .send(ClientJsonRpcMessage::request(
            ClientRequest::DiscoverRequest(discover),
            RequestId::Number(1),
        ))
        .await
        .expect("send discover");
    assert!(matches!(
        client.receive().await,
        Some(ServerJsonRpcMessage::Response(_))
    ));

    client
        .send(ClientJsonRpcMessage::request(
            ClientRequest::ListToolsRequest(ListToolsRequest {
                method: Default::default(),
                params: None,
                extensions: Default::default(),
            }),
            RequestId::Number(2),
        ))
        .await
        .expect("send list tools");
    let Some(ServerJsonRpcMessage::Error(error)) = client.receive().await else {
        panic!("expected invalid params");
    };
    assert_eq!(error.error.code, ErrorCode::INVALID_PARAMS);

    let mut valid_request = list_tools_request(complete_meta());
    if let ClientJsonRpcMessage::Request(request) = &mut valid_request {
        request.id = RequestId::Number(3);
    }
    client
        .send(valid_request)
        .await
        .expect("send valid list tools");
    assert!(matches!(
        client.receive().await,
        Some(ServerJsonRpcMessage::Response(_))
    ));

    let running = server_task.await.expect("server task");

    client
        .send(ClientJsonRpcMessage::request(
            ClientRequest::ListToolsRequest(ListToolsRequest {
                method: Default::default(),
                params: None,
                extensions: Default::default(),
            }),
            RequestId::Number(4),
        ))
        .await
        .expect("send list tools without metadata after inline selection");
    let Some(ServerJsonRpcMessage::Error(error)) = client.receive().await else {
        panic!("expected invalid params");
    };
    assert_eq!(error.error.code, ErrorCode::INVALID_PARAMS);

    running.cancel().await.expect("cancel server");
}

#[derive(Clone)]
struct ContextServer {
    seen_clients: Arc<Mutex<Vec<String>>>,
}

impl ServerHandler for ContextServer {
    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListToolsResult, ErrorData>> + MaybeSendFuture + '_ {
        let seen_clients = self.seen_clients.clone();
        async move {
            seen_clients
                .lock()
                .expect("seen clients lock")
                .push(context.client_info().expect("current client info").name);
            Ok(ListToolsResult::default())
        }
    }
}

#[tokio::test]
async fn stateless_server_uses_each_requests_client_context() {
    let seen_clients = Arc::new(Mutex::new(Vec::new()));
    let handler = ContextServer {
        seen_clients: seen_clients.clone(),
    };
    let (server_transport, client_transport) = tokio::io::duplex(4096);
    let server_task = tokio::spawn(async move {
        handler
            .serve(server_transport)
            .await
            .expect("server should start")
    });
    let mut client = IntoTransport::<rmcp::RoleClient, _, _>::into_transport(client_transport);

    client
        .send(list_tools_request(complete_meta_for("first-client")))
        .await
        .expect("send first request");
    assert!(matches!(
        client.receive().await,
        Some(ServerJsonRpcMessage::Response(_))
    ));

    let mut second = list_tools_request(complete_meta_for("second-client"));
    if let ClientJsonRpcMessage::Request(request) = &mut second {
        request.id = RequestId::Number(2);
    }
    client.send(second).await.expect("send second request");
    assert!(matches!(
        client.receive().await,
        Some(ServerJsonRpcMessage::Response(_))
    ));

    assert_eq!(
        *seen_clients.lock().expect("seen clients lock"),
        ["first-client", "second-client"]
    );
    server_task
        .await
        .expect("server task")
        .cancel()
        .await
        .expect("cancel server");
}

#[tokio::test]
async fn stateless_server_rejects_malformed_metadata_without_selecting_lifecycle() {
    let (server_transport, client_transport) = tokio::io::duplex(4096);
    let server_task = tokio::spawn(async move { StatelessServer.serve(server_transport).await });
    let mut client = IntoTransport::<rmcp::RoleClient, _, _>::into_transport(client_transport);

    let mut request = ListToolsRequest {
        method: Default::default(),
        params: None,
        extensions: Default::default(),
    };
    let malformed: RequestMetaObject = serde_json::from_value(serde_json::json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientInfo": "wrong",
        "io.modelcontextprotocol/clientCapabilities": null
    }))
    .unwrap();
    request.extensions.insert(malformed);
    client
        .send(ClientJsonRpcMessage::request(
            ClientRequest::ListToolsRequest(request),
            RequestId::Number(1),
        ))
        .await
        .expect("send list tools");
    let Some(ServerJsonRpcMessage::Error(error)) = client.receive().await else {
        panic!("expected invalid params");
    };
    assert_eq!(error.id, Some(RequestId::Number(1)));
    assert_eq!(error.error.code, ErrorCode::INVALID_PARAMS);
    assert!(
        error
            .error
            .message
            .contains("request _meta is missing or has malformed required fields")
    );
    assert!(
        error
            .error
            .message
            .contains("io.modelcontextprotocol/clientCapabilities")
    );

    let mut valid_request = list_tools_request(complete_meta());
    if let ClientJsonRpcMessage::Request(request) = &mut valid_request {
        request.id = RequestId::Number(2);
    }
    client
        .send(valid_request)
        .await
        .expect("send valid list tools after malformed request");
    assert!(matches!(
        client.receive().await,
        Some(ServerJsonRpcMessage::Response(_))
    ));

    server_task
        .await
        .expect("server task")
        .expect("valid inline request should start the server")
        .cancel()
        .await
        .expect("cancel server");
}

#[derive(Clone)]
struct ProgressServer;

impl ServerHandler for ProgressServer {
    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let progress_token = context
            .meta
            .get_progress_token()
            .expect("progress token in request meta");
        context
            .peer
            .notify_progress(ProgressNotificationParam::new(progress_token, 1.0))
            .await
            .expect("send progress notification");
        Ok(ListToolsResult::default())
    }
}

/// Regression test for issue #1261: the first request of an `initialize`-less
/// session used to be handled outside the service loop, so a handler that sent
/// a notification before returning waited forever for the loop to flush it.
#[tokio::test]
async fn stateless_server_first_request_handler_can_send_notifications() {
    let (server_transport, client_transport) = tokio::io::duplex(4096);
    let server_task = tokio::spawn(async move {
        ProgressServer
            .serve(server_transport)
            .await
            .expect("server should start")
    });
    let mut client = IntoTransport::<rmcp::RoleClient, _, _>::into_transport(client_transport);

    let mut meta = complete_meta();
    meta.set_progress_token(ProgressToken(NumberOrString::Number(7)));
    client
        .send(list_tools_request(meta))
        .await
        .expect("send first request");

    let exchange = async {
        let notification = match client.receive().await {
            Some(ServerJsonRpcMessage::Notification(notification)) => notification.notification,
            other => panic!("expected progress notification before the response, got {other:?}"),
        };
        assert!(
            matches!(notification, ServerNotification::ProgressNotification(_)),
            "expected progress notification, got {notification:?}"
        );
        let response = match client.receive().await {
            Some(ServerJsonRpcMessage::Response(response)) => response,
            other => panic!("expected list tools response, got {other:?}"),
        };
        assert_eq!(response.id, RequestId::Number(1));
    };
    tokio::time::timeout(Duration::from_secs(5), exchange)
        .await
        .expect("first request must not deadlock the server");

    server_task
        .await
        .expect("server task")
        .cancel()
        .await
        .expect("cancel server");
}

/// SEP-2575 and #1347 together: on the 2026-07-28 lifecycle a handshake method
/// is answered Method not found whether or not its params fit, while a method
/// that exists but whose params do not fit is answered Invalid params.
#[tokio::test]
async fn inline_lifecycle_tells_removed_methods_from_invalid_params() {
    let (server_transport, client_transport) = tokio::io::duplex(4096);
    let server_task = tokio::spawn(async move {
        StatelessServer
            .serve(server_transport)
            .await
            .expect("server should start")
    });
    let mut client = IntoTransport::<rmcp::RoleClient, _, _>::into_transport(client_transport);

    let mut discover = DiscoverRequest::new(DiscoverRequestParams {});
    discover.extensions.insert(complete_meta());
    client
        .send(ClientJsonRpcMessage::request(
            ClientRequest::DiscoverRequest(discover),
            RequestId::Number(1),
        ))
        .await
        .expect("send discover");
    assert!(matches!(
        client.receive().await,
        Some(ServerJsonRpcMessage::Response(_))
    ));
    let running = server_task.await.expect("server task");

    let meta = serde_json::to_value(complete_meta()).expect("meta");
    let cases = [
        // Removed on this lifecycle: not found even with params that fit.
        (
            "logging/setLevel",
            serde_json::json!({"level": "debug"}),
            ErrorCode::METHOD_NOT_FOUND,
        ),
        ("ping", serde_json::json!({}), ErrorCode::METHOD_NOT_FOUND),
        (
            "resources/subscribe",
            serde_json::json!({"uri": "file:///a"}),
            ErrorCode::METHOD_NOT_FOUND,
        ),
        (
            "initialize",
            serde_json::json!({"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "c", "version": "1"}}),
            ErrorCode::METHOD_NOT_FOUND,
        ),
        // Removed, and the params do not fit either: still not found.
        (
            "logging/setLevel",
            serde_json::json!({"level": 7}),
            ErrorCode::METHOD_NOT_FOUND,
        ),
        // Served on this lifecycle, params do not fit: invalid params.
        (
            "tools/call",
            serde_json::json!({"name": 7, "arguments": {}}),
            ErrorCode::INVALID_PARAMS,
        ),
        (
            "prompts/get",
            serde_json::json!({"name": 7}),
            ErrorCode::INVALID_PARAMS,
        ),
    ];
    for (i, (method, mut params, code)) in cases.into_iter().enumerate() {
        params["_meta"] = meta.clone();
        let message: ClientJsonRpcMessage = serde_json::from_value(serde_json::json!({
            "jsonrpc": "2.0", "id": i as i64 + 2, "method": method, "params": params
        }))
        .expect("client message");
        client.send(message).await.expect("send request");
        let Some(ServerJsonRpcMessage::Error(error)) = client.receive().await else {
            panic!("{method}: expected an error");
        };
        assert_eq!(error.error.code, code, "{method}: {}", error.error.message);
        assert!(
            error.error.message.contains(method),
            "{method}: message {:?} does not name the method",
            error.error.message
        );
    }

    running.cancel().await.expect("cancel server");
}

/// Before the handshake, an `initialize` whose params do not fit is answered
/// Invalid params and the server keeps waiting; a proper one still completes it.
#[tokio::test]
async fn malformed_initialize_before_handshake_is_invalid_params() {
    let (server_transport, client_transport) = tokio::io::duplex(4096);
    let server_task = tokio::spawn(async move {
        StatelessServer
            .serve(server_transport)
            .await
            .expect("server should start")
    });
    let mut client = IntoTransport::<rmcp::RoleClient, _, _>::into_transport(client_transport);

    let bad: ClientJsonRpcMessage = serde_json::from_value(serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": 7}
    }))
    .expect("client message");
    client.send(bad).await.expect("send malformed initialize");
    let Some(ServerJsonRpcMessage::Error(error)) = client.receive().await else {
        panic!("expected invalid params");
    };
    assert_eq!(error.error.code, ErrorCode::INVALID_PARAMS);
    assert!(
        error.error.message.contains("initialize"),
        "{}",
        error.error.message
    );

    let good: ClientJsonRpcMessage = serde_json::from_value(serde_json::json!({
        "jsonrpc": "2.0", "id": 2, "method": "initialize",
        "params": {"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "c", "version": "1"}}
    }))
    .expect("client message");
    client.send(good).await.expect("send initialize");
    assert!(matches!(
        client.receive().await,
        Some(ServerJsonRpcMessage::Response(_))
    ));
    let running = server_task.await.expect("server task");
    running.cancel().await.expect("cancel server");
}
