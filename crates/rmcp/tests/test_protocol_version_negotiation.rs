//! Tests for protocol version negotiation in the default ServerHandler::initialize impl.
//!
//! Handshake versions are echoed back; every other version falls back to one
//! the server can serve over `initialize`.
#![cfg(not(feature = "local"))]
#![cfg(feature = "client")]

use std::{
    borrow::Cow,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use rmcp::{
    ClientHandler, ErrorData, RoleServer, ServerHandler, ServiceExt,
    model::{
        ClientCapabilities, ClientConfig, ErrorCode, Implementation, InitializeRequestParams,
        InitializeResult, ProtocolVersion, ServerConfig,
    },
    service::{ClientInitializeError, RequestContext},
};

#[derive(Debug, Clone, Default)]
struct EchoServer;

impl ServerHandler for EchoServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::default()
    }
}

/// Every known version whose lifecycle still runs the `initialize` handshake.
/// `2026-07-28` replaced the handshake with per-request metadata, so this is
/// also the list a server that has not implemented that revision supports.
const HANDSHAKE_VERSIONS: &[ProtocolVersion] = &[
    ProtocolVersion::V_2024_11_05,
    ProtocolVersion::V_2025_03_26,
    ProtocolVersion::V_2025_06_18,
    ProtocolVersion::V_2025_11_25,
];

#[derive(Debug, Clone, Default)]
struct NarrowedServer;

impl ServerHandler for NarrowedServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::default()
    }

    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Borrowed(HANDSHAKE_VERSIONS)
    }
}

/// Supports only revisions that have no `initialize` handshake at all.
#[derive(Debug, Clone, Default)]
struct ModernOnlyServer;

const MODERN_ONLY_VERSIONS: &[ProtocolVersion] = &[ProtocolVersion::V_2026_07_28];

impl ServerHandler for ModernOnlyServer {
    fn get_info(&self) -> ServerConfig {
        let mut info = ServerConfig::default();
        info.protocol_version = ProtocolVersion::V_2026_07_28;
        info
    }

    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Borrowed(MODERN_ONLY_VERSIONS)
    }
}

/// Narrows the supported versions *and* overrides `initialize`, so the
/// handler's own answer never runs the default negotiation. The handshake layer
/// must still honor the narrowed list.
#[derive(Debug, Clone, Default)]
struct NarrowedOverridingServer;

impl ServerHandler for NarrowedOverridingServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::default()
    }

    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Borrowed(HANDSHAKE_VERSIONS)
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
    fn get_info(&self) -> ClientConfig {
        let mut info = ClientConfig::default();
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
    negotiate_with(server, client_version)
        .await
        .expect("client should connect")
}

async fn negotiate_with<S: ServerHandler>(
    server: S,
    client_version: ProtocolVersion,
) -> Result<ProtocolVersion, ClientInitializeError> {
    let (server_transport, client_transport) = tokio::io::duplex(4096);

    tokio::spawn(async move {
        if let Ok(running) = server.serve(server_transport).await {
            let _ = running.waiting().await;
        }
    });

    let client = VersionedClient {
        protocol_version: client_version,
    }
    .serve(client_transport)
    .await?;

    let version = client
        .peer_info()
        .expect("peer_info should be set")
        .protocol_version
        .clone();

    client.cancel().await.expect("client should cancel");
    Ok(version)
}

#[tokio::test]
async fn handshake_version_echoed_back() {
    for version in HANDSHAKE_VERSIONS {
        let negotiated = negotiated_version(version.clone()).await;
        assert_eq!(
            negotiated, *version,
            "handshake version {version} should be echoed back"
        );
    }
}

/// `initialize` disappeared in `2026-07-28`, so agreeing to it here would leave
/// the peers speaking a revision that has no handshake at all.
#[tokio::test]
async fn handshake_never_agrees_to_a_version_that_dropped_it() {
    let negotiated = negotiated_version(ProtocolVersion::V_2026_07_28).await;
    assert_eq!(
        negotiated,
        ProtocolVersion::LATEST,
        "a version that dropped the handshake should fall back to the server's own"
    );
}

#[tokio::test]
async fn modern_only_server_rejects_the_handshake() {
    let error = negotiate_with(ModernOnlyServer, ProtocolVersion::V_2026_07_28)
        .await
        .expect_err("a server with no handshake version cannot answer initialize");
    let ClientInitializeError::JsonRpcError(error) = error else {
        panic!("expected a JSON-RPC error, got {error:?}");
    };
    assert_eq!(
        error.code,
        ErrorCode::UNSUPPORTED_PROTOCOL_VERSION,
        "a server with no handshake version should reject initialize"
    );
    assert_eq!(
        error.data,
        Some(serde_json::json!({
            "requested": "2026-07-28",
            "supported": ["2026-07-28"],
        })),
        "the rejection should name the versions the server does support"
    );
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
    for version in HANDSHAKE_VERSIONS {
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

/// Overrides `initialize` to run a side effect, then delegates the version
/// answer back to the SDK with [`ServerHandler::negotiate_initialize`].
#[derive(Debug, Clone, Default)]
struct DelegatingServer {
    initializations: Arc<AtomicUsize>,
}

impl ServerHandler for DelegatingServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::default()
    }

    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Borrowed(HANDSHAKE_VERSIONS)
    }

    async fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, ErrorData> {
        self.initializations.fetch_add(1, Ordering::Relaxed);
        context.peer.set_peer_info(request.clone());
        self.negotiate_initialize(&request)
    }
}

fn initialize_params(protocol_version: ProtocolVersion) -> InitializeRequestParams {
    let mut params = InitializeRequestParams::new(
        ClientCapabilities::default(),
        Implementation::new("test-client", "0.0.0"),
    );
    params.protocol_version = protocol_version;
    params
}

#[test]
fn negotiate_initialize_echoes_a_supported_version() {
    let result = NarrowedServer
        .negotiate_initialize(&initialize_params(ProtocolVersion::V_2025_06_18))
        .expect("a supported handshake version should negotiate");
    assert_eq!(result.protocol_version, ProtocolVersion::V_2025_06_18);
}

#[test]
fn negotiate_initialize_caps_at_supported_versions() {
    let result = NarrowedServer
        .negotiate_initialize(&initialize_params(ProtocolVersion::V_2026_07_28))
        .expect("an unsupported version should fall back rather than fail");
    assert_eq!(result.protocol_version, ProtocolVersion::V_2025_11_25);
}

#[test]
fn negotiate_initialize_keeps_the_rest_of_get_info() {
    let server = NarrowedServer;
    let result = server
        .negotiate_initialize(&initialize_params(ProtocolVersion::V_2026_07_28))
        .expect("an unsupported version should fall back rather than fail");
    assert_eq!(result.capabilities, server.get_info().capabilities);
}

#[test]
fn negotiate_initialize_rejects_when_no_handshake_version_is_supported() {
    let error = ModernOnlyServer
        .negotiate_initialize(&initialize_params(ProtocolVersion::V_2026_07_28))
        .expect_err("a server with no handshake version cannot answer initialize");
    assert_eq!(error.code, ErrorCode::UNSUPPORTED_PROTOCOL_VERSION);
}

#[tokio::test]
async fn delegating_server_negotiates_like_the_default_initialize() {
    let negotiated =
        negotiated_version_with(DelegatingServer::default(), ProtocolVersion::V_2026_07_28).await;
    assert_eq!(
        negotiated,
        ProtocolVersion::V_2025_11_25,
        "an override that delegates should answer what the default initialize would"
    );
}

#[tokio::test]
async fn delegating_server_still_runs_its_own_side_effect() {
    let server = DelegatingServer::default();
    let initializations = Arc::clone(&server.initializations);
    negotiated_version_with(server, ProtocolVersion::V_2025_06_18).await;
    assert_eq!(initializations.load(Ordering::Relaxed), 1);
}
