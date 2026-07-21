// cargo test --features "server client" --package rmcp test_client_initialization
#![cfg(all(feature = "client", not(feature = "local")))]

mod common;

use std::borrow::Cow;

use common::handlers::TestClientHandler;
use rmcp::{
    ServiceExt,
    model::{
        ClientJsonRpcMessage, ErrorCode, ErrorData, InitializeResult, JsonRpcError,
        JsonRpcVersion2_0, RequestId, ServerCapabilities, ServerJsonRpcMessage, ServerResult,
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
        let _init_request = server.receive().await;

        let error_msg = ServerJsonRpcMessage::Error(JsonRpcError {
            jsonrpc: JsonRpcVersion2_0,
            id: Some(RequestId::Number(1)),
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
