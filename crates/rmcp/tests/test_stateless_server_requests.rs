#![cfg(all(feature = "server", not(feature = "local")))]

use std::sync::{Arc, Mutex};

use rmcp::{
    ServerHandler, ServiceExt,
    model::{
        ClientCapabilities, ClientJsonRpcMessage, ClientRequest, DiscoverRequest,
        DiscoverRequestParams, ErrorCode, ErrorData, Implementation, ListToolsRequest,
        ListToolsResult, PaginatedRequestParams, ProtocolVersion, RequestId, RequestMetaObject,
        ServerJsonRpcMessage,
    },
    service::{MaybeSendFuture, RequestContext, RoleServer, ServerInitializeError},
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

    server_task
        .await
        .expect("server task")
        .cancel()
        .await
        .expect("cancel server");
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
async fn stateless_server_rejects_malformed_metadata_opener() {
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
    let Err(error) = server_task.await.expect("server task") else {
        panic!("malformed opener should not start a session");
    };
    assert!(matches!(
        error,
        ServerInitializeError::ExpectedInitializeRequest(Some(_))
    ));
}
