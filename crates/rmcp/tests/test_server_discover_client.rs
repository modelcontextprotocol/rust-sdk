#![cfg(all(feature = "client", not(feature = "local")))]

use rmcp::{
    ClientHandler, ClientLifecycleMode, ClientServiceExt, ServerHandler, ServiceExt,
    model::{
        ClientCapabilities, Implementation, ProtocolVersion, RequestMetaObject, ServerCapabilities,
        ServerInfo,
    },
    select_protocol_version,
};

#[derive(Clone, Default)]
struct DiscoveryServer;

impl ServerHandler for DiscoveryServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("discovery-server", "1.0.0"))
    }
}

#[derive(Clone, Default)]
struct DiscoveryClient;

impl ClientHandler for DiscoveryClient {}

#[test]
fn select_protocol_version_uses_client_preference_order() {
    let selected = select_protocol_version(
        &[ProtocolVersion::V_2026_07_28, ProtocolVersion::V_2025_11_25],
        &[ProtocolVersion::V_2025_11_25, ProtocolVersion::V_2026_07_28],
    );

    assert_eq!(selected, Some(ProtocolVersion::V_2026_07_28));
}

#[test]
fn select_protocol_version_returns_none_without_overlap() {
    let selected = select_protocol_version(
        &[ProtocolVersion::V_2026_07_28],
        &[ProtocolVersion::V_2025_11_25],
    );

    assert_eq!(selected, None);
}

#[tokio::test]
async fn client_discover_helper_returns_typed_result() {
    let (server_transport, client_transport) = tokio::io::duplex(4096);
    tokio::spawn(async move {
        let _ = DiscoveryServer
            .serve(server_transport)
            .await
            .expect("server should start")
            .waiting()
            .await;
    });
    let client = DiscoveryClient
        .serve(client_transport)
        .await
        .expect("client should connect");
    let mut meta = RequestMetaObject::new();
    meta.set_protocol_version(ProtocolVersion::V_2026_07_28);
    meta.set_client_info(Implementation::new("discovery-client", "1.0.0"));
    meta.set_client_capabilities(ClientCapabilities::default());

    let result = client
        .discover(meta)
        .await
        .expect("discover should succeed");

    assert_eq!(
        result.server_info,
        Implementation::new("discovery-server", "1.0.0")
    );
    client.cancel().await.expect("client should cancel");
}

#[tokio::test]
async fn discover_startup_accepts_historical_preferred_protocol_version() {
    let (server_transport, client_transport) = tokio::io::duplex(4096);
    let server_task = tokio::spawn(async move {
        DiscoveryServer
            .serve(server_transport)
            .await
            .expect("server should accept discovery")
    });

    let client = DiscoveryClient
        .serve_with_lifecycle(
            client_transport,
            ClientLifecycleMode::Discover {
                preferred_versions: vec![ProtocolVersion::V_2025_11_25],
            },
        )
        .await
        .expect("discovery should preserve its required result discriminator");

    assert_eq!(
        client
            .peer_info()
            .expect("discovery should store server information")
            .protocol_version,
        ProtocolVersion::V_2025_11_25
    );
    client.cancel().await.expect("client should cancel");
    let server = server_task.await.expect("server task should complete");
    server.cancel().await.expect("server should cancel");
}
