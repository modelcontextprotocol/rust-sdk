//! SEP-2164: the resource-not-found error code follows the negotiated protocol version.
//!
//! `2026-07-28` and newer get the standard `INVALID_PARAMS` (-32602); older versions
//! keep the legacy `RESOURCE_NOT_FOUND` (-32002).
#![cfg(not(feature = "local"))]
#![cfg(feature = "client")]

use rmcp::{
    ClientHandler, RoleClient, RoleServer, ServerHandler, ServiceError,
    model::{
        ClientInfo, ErrorCode, ErrorData, ProtocolVersion, ReadResourceRequestParams,
        ReadResourceResponse, ServerInfo,
    },
    service::{RequestContext, serve_directly},
};

#[derive(Debug, Clone, Default)]
struct ResourceServer;

impl ServerHandler for ResourceServer {
    async fn read_resource(
        &self,
        _request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        Err(ErrorData::resource_not_found("resource not found", None))
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

/// Wires the pair up directly on `client_version`. `2026-07-28` removed the
/// `initialize` handshake, so a peer on that revision is reached the way the
/// discover lifecycle leaves one: with the version already agreed.
async fn not_found_code(client_version: ProtocolVersion) -> ErrorCode {
    let (server_transport, client_transport) = tokio::io::duplex(4096);

    let client_handler = VersionedClient {
        protocol_version: client_version.clone(),
    };
    let mut server_peer_info = ServerInfo::default();
    server_peer_info.protocol_version = client_version;

    let server = serve_directly::<RoleServer, _, _, _, _>(
        ResourceServer,
        server_transport,
        Some(client_handler.get_info()),
    );
    let server_handle = tokio::spawn(async move {
        server.waiting().await?;
        anyhow::Ok(())
    });

    let client = serve_directly::<RoleClient, _, _, _, _>(
        client_handler,
        client_transport,
        Some(server_peer_info.into()),
    );

    let error = client
        .read_resource(ReadResourceRequestParams::new("missing://resource"))
        .await
        .expect_err("missing resource should error");

    let code = match error {
        ServiceError::McpError(data) => data.code,
        other => panic!("expected McpError, got: {other:?}"),
    };

    client.cancel().await.expect("client should cancel");
    server_handle.await.expect("server task").expect("server");
    code
}

#[tokio::test]
async fn legacy_version_gets_resource_not_found_code() {
    assert_eq!(
        not_found_code(ProtocolVersion::V_2025_11_25).await,
        ErrorCode::RESOURCE_NOT_FOUND,
    );
}

#[tokio::test]
async fn sep_2164_version_gets_invalid_params_code() {
    assert_eq!(
        not_found_code(ProtocolVersion::V_2026_07_28).await,
        ErrorCode::INVALID_PARAMS,
    );
}
