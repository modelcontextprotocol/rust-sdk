// cargo test --features "client" --package rmcp -- server_init
#![cfg(all(feature = "client", not(feature = "local")))]
mod common;

use common::handlers::TestServer;
use rmcp::{
    ServiceExt,
    model::{ClientJsonRpcMessage, ServerJsonRpcMessage, ServerResult},
    service::ServerInitializeError,
    transport::{IntoTransport, Transport},
};

fn msg(raw: &str) -> ClientJsonRpcMessage {
    serde_json::from_str(raw).expect("invalid test message JSON")
}

fn init_request() -> ClientJsonRpcMessage {
    msg(r#"{
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": { "name": "test-client", "version": "0.0.1" }
        }
    }"#)
}

fn initialized_notification() -> ClientJsonRpcMessage {
    msg(r#"{ "jsonrpc": "2.0", "method": "notifications/initialized" }"#)
}

fn set_level_request(id: u64) -> ClientJsonRpcMessage {
    msg(&format!(
        r#"{{ "jsonrpc": "2.0", "id": {id}, "method": "logging/setLevel", "params": {{ "level": "info" }} }}"#
    ))
}

fn ping_request(id: u64) -> ClientJsonRpcMessage {
    msg(&format!(
        r#"{{ "jsonrpc": "2.0", "id": {id}, "method": "ping" }}"#
    ))
}

fn list_tools_request(id: u64) -> ClientJsonRpcMessage {
    msg(&format!(
        r#"{{ "jsonrpc": "2.0", "id": {id}, "method": "tools/list" }}"#
    ))
}

async fn do_initialize(client: &mut impl Transport<rmcp::RoleClient>) {
    client.send(init_request()).await.unwrap();
    let _response = client.receive().await.unwrap();
}

// Server responds with EmptyResult to setLevel received before initialized.
#[tokio::test]
async fn server_init_set_level_response_is_empty_result() {
    let (server_transport, client_transport) = tokio::io::duplex(4096);
    let _server = tokio::spawn(async move { TestServer::new().serve(server_transport).await });
    let mut client = IntoTransport::<rmcp::RoleClient, _, _>::into_transport(client_transport);

    do_initialize(&mut client).await;
    client.send(set_level_request(2)).await.unwrap();

    let response = client.receive().await.unwrap();
    assert!(
        matches!(
            response,
            ServerJsonRpcMessage::Response(ref r)
                if matches!(r.result, ServerResult::EmptyResult(_))
        ),
        "expected EmptyResult for setLevel, got: {response:?}"
    );
}

// Server initializes successfully when setLevel is sent before the initialized notification.
#[tokio::test]
async fn server_init_succeeds_after_set_level_before_initialized() {
    let (server_transport, client_transport) = tokio::io::duplex(4096);
    let server_handle =
        tokio::spawn(async move { TestServer::new().serve(server_transport).await });
    let mut client = IntoTransport::<rmcp::RoleClient, _, _>::into_transport(client_transport);

    do_initialize(&mut client).await;
    client.send(set_level_request(2)).await.unwrap();
    let _response = client.receive().await.unwrap();
    client.send(initialized_notification()).await.unwrap();

    let result = server_handle.await.unwrap();
    assert!(
        result.is_ok(),
        "server should initialize successfully after setLevel"
    );
    result.unwrap().cancel().await.unwrap();
}

// Server responds with EmptyResult to ping received before initialize request.
#[tokio::test]
async fn server_init_ping_response_is_empty_result_before_initialize() {
    let (server_transport, client_transport) = tokio::io::duplex(4096);
    let _server = tokio::spawn(async move { TestServer::new().serve(server_transport).await });
    let mut client = IntoTransport::<rmcp::RoleClient, _, _>::into_transport(client_transport);

    client.send(ping_request(1)).await.unwrap();

    let response = client.receive().await.unwrap();
    assert!(
        matches!(
            response,
            ServerJsonRpcMessage::Response(ref r)
                if matches!(r.result, ServerResult::EmptyResult(_))
        ),
        "expected EmptyResult for pre-initialize ping, got: {response:?}"
    );
}

// Server initializes successfully when ping is sent before the initialize request.
#[tokio::test]
async fn server_init_succeeds_after_ping_before_initialize() {
    let (server_transport, client_transport) = tokio::io::duplex(4096);
    let server_handle =
        tokio::spawn(async move { TestServer::new().serve(server_transport).await });
    let mut client = IntoTransport::<rmcp::RoleClient, _, _>::into_transport(client_transport);

    client.send(ping_request(1)).await.unwrap();
    let _pong = client.receive().await.unwrap();
    do_initialize(&mut client).await;
    client.send(initialized_notification()).await.unwrap();

    let result = server_handle.await.unwrap();
    assert!(
        result.is_ok(),
        "server should initialize successfully after pre-initialize ping"
    );
    result.unwrap().cancel().await.unwrap();
}

// Server responds with EmptyResult to ping received before initialized.
#[tokio::test]
async fn server_init_ping_response_is_empty_result() {
    let (server_transport, client_transport) = tokio::io::duplex(4096);
    let _server = tokio::spawn(async move { TestServer::new().serve(server_transport).await });
    let mut client = IntoTransport::<rmcp::RoleClient, _, _>::into_transport(client_transport);

    do_initialize(&mut client).await;
    client.send(ping_request(2)).await.unwrap();

    let response = client.receive().await.unwrap();
    assert!(
        matches!(
            response,
            ServerJsonRpcMessage::Response(ref r)
                if matches!(r.result, ServerResult::EmptyResult(_))
        ),
        "expected EmptyResult for ping, got: {response:?}"
    );
}

// Server initializes successfully when ping is sent before the initialized notification.
#[tokio::test]
async fn server_init_succeeds_after_ping_before_initialized() {
    let (server_transport, client_transport) = tokio::io::duplex(4096);
    let server_handle =
        tokio::spawn(async move { TestServer::new().serve(server_transport).await });
    let mut client = IntoTransport::<rmcp::RoleClient, _, _>::into_transport(client_transport);

    do_initialize(&mut client).await;
    client.send(ping_request(2)).await.unwrap();
    let _response = client.receive().await.unwrap();
    client.send(initialized_notification()).await.unwrap();

    let result = server_handle.await.unwrap();
    assert!(
        result.is_ok(),
        "server should initialize successfully after ping"
    );
    result.unwrap().cancel().await.unwrap();
}

// Server returns ExpectedInitializedNotification for any other message before initialized.
#[tokio::test]
async fn server_init_rejects_unexpected_message_before_initialized() {
    let (server_transport, client_transport) = tokio::io::duplex(4096);
    let server_handle =
        tokio::spawn(async move { TestServer::new().serve(server_transport).await });
    let mut client = IntoTransport::<rmcp::RoleClient, _, _>::into_transport(client_transport);

    do_initialize(&mut client).await;
    client.send(list_tools_request(2)).await.unwrap();

    let result = server_handle.await.unwrap();
    assert!(
        matches!(
            result,
            Err(ServerInitializeError::ExpectedInitializedNotification(_))
        ),
        "expected ExpectedInitializedNotification error"
    );
}
