// cargo test --features "server client" --package rmcp test_client_initialization
mod common;

use std::borrow::Cow;

use common::handlers::TestClientHandler;
use rmcp::{
    ServiceExt,
    model::{
        ErrorCode, ErrorData, JsonRpcError, JsonRpcVersion2_0, RequestId, ServerJsonRpcMessage,
    },
    transport::{IntoTransport, Transport},
};

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
            id: RequestId::Number(1),
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
