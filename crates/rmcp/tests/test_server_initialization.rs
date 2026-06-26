// cargo test --features "client" --package rmcp -- server_init
#![cfg(all(feature = "client", not(feature = "local")))]
mod common;

use common::handlers::TestServer;
use rmcp::{
    ServerHandler, ServiceExt,
    model::{
        ClientJsonRpcMessage, ProtocolVersion, ServerCapabilities, ServerInfo,
        ServerJsonRpcMessage, ServerResult,
    },
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

// Server handles setLevel sent before initialized notification (processed by serve_inner).
#[tokio::test]
async fn server_init_set_level_response_is_empty_result() {
    let (server_transport, client_transport) = tokio::io::duplex(4096);
    let _server = tokio::spawn(async move { TestServer::new().serve(server_transport).await });
    let mut client = IntoTransport::<rmcp::RoleClient, _, _>::into_transport(client_transport);

    do_initialize(&mut client).await;
    client.send(set_level_request(2)).await.unwrap();

    // The handler may send logging notifications before the response;
    // skip notifications to find the EmptyResult response.
    let response = loop {
        let msg = client.receive().await.unwrap();
        if matches!(msg, ServerJsonRpcMessage::Response(_)) {
            break msg;
        }
    };
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
    // Skip notifications until we get the response
    loop {
        let msg = client.receive().await.unwrap();
        if matches!(msg, ServerJsonRpcMessage::Response(_)) {
            break;
        }
    }
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

// Server buffers tools/list sent before initialized and processes it after initialization.
#[tokio::test]
async fn server_init_buffers_request_before_initialized() {
    let (server_transport, client_transport) = tokio::io::duplex(4096);
    let server_handle =
        tokio::spawn(async move { TestServer::new().serve(server_transport).await });
    let mut client = IntoTransport::<rmcp::RoleClient, _, _>::into_transport(client_transport);

    do_initialize(&mut client).await;
    // Send tools/list before initialized notification
    client.send(list_tools_request(2)).await.unwrap();
    // Now send initialized notification
    client.send(initialized_notification()).await.unwrap();

    // The buffered tools/list should be processed — expect a response
    let response = client.receive().await.unwrap();
    assert!(
        matches!(response, ServerJsonRpcMessage::Response(_)),
        "expected response for buffered tools/list, got: {response:?}"
    );

    let result = server_handle.await.unwrap();
    assert!(
        result.is_ok(),
        "server should initialize successfully when buffering pre-init messages"
    );
    result.unwrap().cancel().await.unwrap();
}

fn init_request_with_version(v: &str) -> ClientJsonRpcMessage {
    msg(&format!(
        r#"{{
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {{
                "protocolVersion": "{v}",
                "capabilities": {{}},
                "clientInfo": {{ "name": "test-client", "version": "0.0.1" }}
            }}
        }}"#
    ))
}

async fn negotiate_version<H>(handler: H, client_version: &str) -> ProtocolVersion
where
    H: ServerHandler + 'static,
{
    let (server_transport, client_transport) = tokio::io::duplex(4096);
    let _server = tokio::spawn(async move { handler.serve(server_transport).await });
    let mut client = IntoTransport::<rmcp::RoleClient, _, _>::into_transport(client_transport);

    client
        .send(init_request_with_version(client_version))
        .await
        .unwrap();
    let response = client.receive().await.unwrap();
    let ServerJsonRpcMessage::Response(r) = response else {
        panic!("expected initialize response, got {response:?}");
    };
    let ServerResult::InitializeResult(init) = r.result else {
        panic!("expected InitializeResult");
    };
    init.protocol_version
}

#[tokio::test]
async fn server_echoes_client_protocol_version_when_known_old() {
    let negotiated = negotiate_version(TestServer::new(), "2024-11-05").await;
    assert_eq!(negotiated, ProtocolVersion::V_2024_11_05);
}

#[tokio::test]
async fn server_echoes_client_protocol_version_when_latest() {
    let negotiated = negotiate_version(TestServer::new(), "2025-11-25").await;
    assert_eq!(negotiated, ProtocolVersion::LATEST);
}

#[tokio::test]
async fn server_falls_back_when_client_protocol_version_unknown() {
    let negotiated = negotiate_version(TestServer::new(), "2099-99-99").await;
    assert_eq!(negotiated, ProtocolVersion::LATEST);
}

struct PinnedServer;

impl ServerHandler for PinnedServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().build())
            .with_protocol_version(ProtocolVersion::V_2025_06_18)
    }
}

#[tokio::test]
async fn server_pinned_version_does_not_override_known_client_request() {
    let negotiated = negotiate_version(PinnedServer, "2025-11-25").await;
    assert_eq!(negotiated, ProtocolVersion::LATEST);
}

#[tokio::test]
async fn server_pinned_version_used_as_fallback_for_unknown_client_request() {
    let negotiated = negotiate_version(PinnedServer, "2099-99-99").await;
    assert_eq!(negotiated, ProtocolVersion::V_2025_06_18);
}

fn duplicate_init_request(id: u64, version: &str) -> ClientJsonRpcMessage {
    msg(&format!(
        r#"{{
            "jsonrpc": "2.0",
            "id": {id},
            "method": "initialize",
            "params": {{
                "protocolVersion": "{version}",
                "capabilities": {{ "sampling": {{}} }},
                "clientInfo": {{ "name": "renegotiated-client", "version": "9.9.9" }}
            }}
        }}"#
    ))
}

#[tokio::test]
async fn server_accepts_duplicate_initialize() {
    let (server_transport, client_transport) = tokio::io::duplex(4096);
    let _server = tokio::spawn(async move { TestServer::new().serve(server_transport).await });
    let mut client = IntoTransport::<rmcp::RoleClient, _, _>::into_transport(client_transport);

    do_initialize(&mut client).await;
    client.send(initialized_notification()).await.unwrap();

    client
        .send(duplicate_init_request(2, "2025-11-25"))
        .await
        .unwrap();
    let response = client.receive().await.unwrap();
    assert!(
        matches!(response, ServerJsonRpcMessage::Response(_)),
        "expected successful InitializeResult, got: {response:?}"
    );
}

#[tokio::test]
async fn server_session_remains_usable_after_renegotiation() {
    let (server_transport, client_transport) = tokio::io::duplex(4096);
    let _server = tokio::spawn(async move { TestServer::new().serve(server_transport).await });
    let mut client = IntoTransport::<rmcp::RoleClient, _, _>::into_transport(client_transport);

    do_initialize(&mut client).await;
    client.send(initialized_notification()).await.unwrap();
    client
        .send(duplicate_init_request(2, "2025-11-25"))
        .await
        .unwrap();
    let _renegotiated = client.receive().await.unwrap();

    client.send(ping_request(3)).await.unwrap();
    let pong = client.receive().await.unwrap();
    assert!(
        matches!(
            pong,
            ServerJsonRpcMessage::Response(ref r)
                if matches!(r.result, ServerResult::EmptyResult(_))
        ),
        "expected EmptyResult ping after renegotiation, got: {pong:?}"
    );
}

// Server buffers multiple requests before initialized and processes them in order.
#[tokio::test]
async fn server_init_buffers_multiple_requests_before_initialized() {
    let (server_transport, client_transport) = tokio::io::duplex(4096);
    let server_handle =
        tokio::spawn(async move { TestServer::new().serve(server_transport).await });
    let mut client = IntoTransport::<rmcp::RoleClient, _, _>::into_transport(client_transport);

    do_initialize(&mut client).await;
    // Send two requests before initialized
    client.send(list_tools_request(2)).await.unwrap();
    client.send(ping_request(3)).await.unwrap();
    // Now send initialized notification
    client.send(initialized_notification()).await.unwrap();

    // Both buffered messages should get responses
    let response1 = client.receive().await.unwrap();
    let response2 = client.receive().await.unwrap();
    assert!(
        matches!(response1, ServerJsonRpcMessage::Response(_)),
        "expected response for first buffered message, got: {response1:?}"
    );
    assert!(
        matches!(response2, ServerJsonRpcMessage::Response(_)),
        "expected response for second buffered message, got: {response2:?}"
    );

    let result = server_handle.await.unwrap();
    assert!(
        result.is_ok(),
        "server should initialize successfully with multiple buffered messages"
    );
    result.unwrap().cancel().await.unwrap();
}
