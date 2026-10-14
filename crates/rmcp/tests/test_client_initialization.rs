// cargo test --features "server client" --package rmcp test_client_initialization
#![cfg(all(feature = "client", not(feature = "local")))]

mod common;

use std::borrow::Cow;

use common::handlers::TestClientHandler;
use rmcp::{
    ServiceExt,
    model::{
        ClientConfig, ClientJsonRpcMessage, ClientResult, ErrorCode, ErrorData, InitializeResult,
        JsonRpcError, JsonRpcVersion2_0, PingRequest, ProtocolVersion, RequestId,
        ServerCapabilities, ServerJsonRpcMessage, ServerRequest, ServerResult,
    },
    transport::{IntoTransport, Transport},
};

fn stringify_numeric_id(id: RequestId) -> RequestId {
    let RequestId::Number(id) = id else {
        panic!("expected a numeric request ID");
    };
    RequestId::String(id.to_string().into())
}

#[tokio::test]
async fn client_initialization_accepts_stringified_numeric_response_id() {
    let (server_transport, client_transport) = tokio::io::duplex(1024);
    let mut server = IntoTransport::<rmcp::RoleServer, _, _>::into_transport(server_transport);
    let server_task = tokio::spawn(async move {
        let ClientJsonRpcMessage::Request(request) =
            server.receive().await.expect("expected initialize request")
        else {
            panic!("expected initialize request");
        };
        server
            .send(ServerJsonRpcMessage::response(
                ServerResult::InitializeResult(
                    InitializeResult::new(ServerCapabilities::default()),
                ),
                stringify_numeric_id(request.id),
            ))
            .await
            .expect("send initialize response");
        assert!(matches!(
            server.receive().await,
            Some(ClientJsonRpcMessage::Notification(_))
        ));
    });

    let client = TestClientHandler::new(true, true)
        .serve(client_transport)
        .await
        .expect("client should accept stringified initialize response ID");
    assert!(
        client
            .peer_info()
            .expect("peer info should be retained")
            .server_info
            .is_some(),
        "initialize always provides a server implementation identity"
    );
    client.cancel().await.expect("cancel client");
    server_task.await.expect("server task");
}

#[tokio::test]
async fn client_correlates_stringified_numeric_response_id() {
    let (server_transport, client_transport) = tokio::io::duplex(1024);
    let mut server = IntoTransport::<rmcp::RoleServer, _, _>::into_transport(server_transport);
    let server_task = tokio::spawn(async move {
        let ClientJsonRpcMessage::Request(initialize) =
            server.receive().await.expect("expected initialize request")
        else {
            panic!("expected initialize request");
        };
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

        let ClientJsonRpcMessage::Request(request) =
            server.receive().await.expect("expected tools/list request")
        else {
            panic!("expected tools/list request");
        };
        server
            .send(ServerJsonRpcMessage::response(
                ServerResult::ListToolsResult(Default::default()),
                stringify_numeric_id(request.id),
            ))
            .await
            .expect("send tools/list response");
    });

    let client = TestClientHandler::new(true, true)
        .serve(client_transport)
        .await
        .expect("initialize client");
    client
        .list_tools(None)
        .await
        .expect("client should correlate stringified response ID");
    client.cancel().await.expect("cancel client");
    server_task.await.expect("server task");
}

#[tokio::test]
async fn test_client_init_handles_jsonrpc_error() {
    let (server_transport, client_transport) = tokio::io::duplex(1024);
    let mut server = IntoTransport::<rmcp::RoleServer, _, _>::into_transport(server_transport);

    let client_handle = tokio::spawn(async move {
        TestClientHandler::new(true, true)
            .serve(client_transport)
            .await
    });

    tokio::spawn(async move {
        let request = server.receive().await;
        // Echo the request's own id back on the error so it correlates: an
        // uncorrelated id would surface as `UncorrelatedErrorResponse`
        // instead of the `JsonRpcError` this test exercises.
        let request_id = request
            .and_then(|message| message.into_request())
            .map(|(_, id)| id)
            .expect("client sent an initialize request");

        let error_msg = ServerJsonRpcMessage::Error(JsonRpcError {
            jsonrpc: JsonRpcVersion2_0,
            id: Some(request_id),
            error: ErrorData {
                code: ErrorCode(-32600),
                message: Cow::Borrowed("Invalid Request"),
                data: None,
            },
        });
        let _: Result<(), _> = server.send(error_msg).await;
    });

    let result = client_handle.await.unwrap();

    assert!(result.is_err());
    match result {
        Err(rmcp::service::ClientInitializeError::JsonRpcError(error_data)) => {
            assert_eq!(error_data.code, ErrorCode(-32600));
            assert_eq!(error_data.message, "Invalid Request");
        }
        _ => panic!("Expected ClientInitializeError::JsonRpcError"),
    }
}

#[tokio::test]
async fn client_answers_server_ping_during_initialize() {
    let (server_transport, client_transport) = tokio::io::duplex(4096);
    let mut server = IntoTransport::<rmcp::RoleServer, _, _>::into_transport(server_transport);
    let server_task = tokio::spawn(async move {
        let ClientJsonRpcMessage::Request(initialize) =
            server.receive().await.expect("expected initialize request")
        else {
            panic!("expected initialize request");
        };
        let ping_id = RequestId::String("server-ping".into());
        server
            .send(ServerJsonRpcMessage::request(
                ServerRequest::PingRequest(PingRequest::default()),
                ping_id.clone(),
            ))
            .await
            .expect("send ping");
        server
            .send(ServerJsonRpcMessage::response(
                ServerResult::InitializeResult(
                    InitializeResult::new(ServerCapabilities::default())
                        .with_protocol_version(ProtocolVersion::LATEST_WITH_INITIALIZE),
                ),
                initialize.id,
            ))
            .await
            .expect("send initialize response");

        let Some(ClientJsonRpcMessage::Response(response)) = server.receive().await else {
            panic!("expected the ping response before notifications/initialized");
        };
        assert_eq!(response.id, ping_id);
        assert!(matches!(response.result, ClientResult::EmptyResult(_)));
        assert!(matches!(
            server.receive().await,
            Some(ClientJsonRpcMessage::Notification(_))
        ));
    });

    let client = ClientConfig::default()
        .with_protocol_version(ProtocolVersion::LATEST_WITH_INITIALIZE)
        .serve(client_transport)
        .await
        .expect("initialize client");
    client.cancel().await.expect("cancel client");
    server_task.await.expect("server task");
}
