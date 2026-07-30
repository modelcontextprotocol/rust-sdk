//! Tests for protocol version negotiation in the default ServerHandler::initialize impl.
//!
//! Known versions are echoed back; unknown versions fall back to LATEST.
#![cfg(not(feature = "local"))]
#![cfg(feature = "client")]

use std::borrow::Cow;

use rmcp::{
    ClientHandler, ErrorData, RoleServer, ServerHandler, ServiceExt,
    model::{ClientInfo, InitializeRequestParams, InitializeResult, ProtocolVersion, ServerInfo},
    service::RequestContext,
};

#[derive(Debug, Clone, Default)]
struct EchoServer;

impl ServerHandler for EchoServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::default()
    }
}

/// Every known version except `2026-07-28`, standing in for a server that has
/// not implemented that revision.
const NARROWED_VERSIONS: &[ProtocolVersion] = &[
    ProtocolVersion::V_2024_11_05,
    ProtocolVersion::V_2025_03_26,
    ProtocolVersion::V_2025_06_18,
    ProtocolVersion::V_2025_11_25,
];

#[derive(Debug, Clone, Default)]
struct NarrowedServer;

impl ServerHandler for NarrowedServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::default()
    }

    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Borrowed(NARROWED_VERSIONS)
    }
}

/// Narrows the supported versions *and* overrides `initialize`, so the
/// handler's own answer never runs the default negotiation. The handshake layer
/// must still honor the narrowed list.
#[derive(Debug, Clone, Default)]
struct NarrowedOverridingServer;

impl ServerHandler for NarrowedOverridingServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::default()
    }

    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Borrowed(NARROWED_VERSIONS)
    }

    async fn initialize(
        &self,
        _request: InitializeRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, ErrorData> {
        Ok(self.get_info())
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
    negotiated_version_with(EchoServer, client_version).await
}

async fn negotiated_version_with<S: ServerHandler>(
    server: S,
    client_version: ProtocolVersion,
) -> ProtocolVersion {
    let (server_transport, client_transport) = tokio::io::duplex(4096);

    tokio::spawn(async move {
        let _ = server
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

#[tokio::test]
async fn narrowed_server_still_echoes_versions_it_supports() {
    for version in NARROWED_VERSIONS {
        let negotiated = negotiated_version_with(NarrowedServer, version.clone()).await;
        assert_eq!(
            negotiated, *version,
            "supported version {version} should be echoed back"
        );
    }
}

#[tokio::test]
async fn narrowed_server_does_not_agree_to_version_it_excludes() {
    let negotiated = negotiated_version_with(NarrowedServer, ProtocolVersion::V_2026_07_28).await;
    assert_eq!(
        negotiated,
        ProtocolVersion::V_2025_11_25,
        "a version outside supported_protocol_versions should not be echoed back"
    );
}

#[tokio::test]
async fn narrowed_server_caps_even_when_it_overrides_initialize() {
    let negotiated =
        negotiated_version_with(NarrowedOverridingServer, ProtocolVersion::V_2026_07_28).await;
    assert_eq!(
        negotiated,
        ProtocolVersion::V_2025_11_25,
        "the handshake layer should not raise the version above what the server supports"
    );
}
