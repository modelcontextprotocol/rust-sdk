//! Tests for protocol version negotiation in the default ServerHandler::initialize impl.
//!
//! Known versions are echoed back; unknown versions fall back to LATEST.
#![cfg(not(feature = "local"))]
#![cfg(feature = "client")]

use rmcp::{
    ClientHandler, ServerHandler, ServiceExt,
    model::{ClientInfo, ProtocolVersion, ServerInfo},
};

#[derive(Debug, Clone, Default)]
struct EchoServer;

impl ServerHandler for EchoServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::default()
    }
}

#[derive(Debug, Clone)]
struct VersionedClient {
    protocol_version: ProtocolVersion,
}

impl ClientHandler for VersionedClient {
    fn get_info(&self) -> ClientInfo {
        let mut info = ClientInfo::default();
        info.protocol_version = self.protocol_version.clone();
        info
    }
}

async fn negotiated_version(client_version: ProtocolVersion) -> ProtocolVersion {
    let (server_transport, client_transport) = tokio::io::duplex(4096);

    tokio::spawn(async move {
        let _ = EchoServer
            .serve(server_transport)
            .await
            .expect("server should start")
            .waiting()
            .await;
    });

    let client = VersionedClient {
        protocol_version: client_version,
    }
    .serve(client_transport)
    .await
    .expect("client should connect");

    let version = client
        .peer_info()
        .expect("peer_info should be set")
        .protocol_version
        .clone();

    client.cancel().await.expect("client should cancel");
    version
}

#[tokio::test]
async fn known_version_echoed_back() {
    for version in ProtocolVersion::KNOWN_VERSIONS {
        let negotiated = negotiated_version(version.clone()).await;
        assert_eq!(
            negotiated, *version,
            "known version {version} should be echoed back"
        );
    }
}

#[tokio::test]
async fn unknown_version_falls_back_to_latest() {
    let unknown: ProtocolVersion = serde_json::from_str(r#""1999-01-01""#).unwrap();
    let negotiated = negotiated_version(unknown).await;
    assert_eq!(
        negotiated,
        ProtocolVersion::LATEST,
        "unknown version should fall back to LATEST"
    );
}
