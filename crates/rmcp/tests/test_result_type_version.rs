//! SEP-2322: the `resultType` discriminator follows the negotiated protocol version.
//!
//! Peers negotiating `2026-07-28` or newer receive `resultType: "complete"` on
//! ordinary results; older peers keep the legacy wire shape without the field.
#![cfg(not(feature = "local"))]
#![cfg(feature = "client")]

use rmcp::{
    ClientHandler, RoleClient, RoleServer, ServerHandler,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ClientInfo, ContentBlock,
        ErrorData, ProtocolVersion, ResultType, ServerInfo,
    },
    service::{RequestContext, serve_directly},
};

#[derive(Debug, Clone, Default)]
struct ToolServer;

impl ServerHandler for ToolServer {
    async fn call_tool(
        &self,
        _request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        Ok(CallToolResult::success(vec![ContentBlock::text("ok")]).into())
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
async fn call_tool_result_type(client_version: ProtocolVersion) -> Option<ResultType> {
    let (server_transport, client_transport) = tokio::io::duplex(4096);

    let client_handler = VersionedClient {
        protocol_version: client_version.clone(),
    };
    let mut server_peer_info = ServerInfo::default();
    server_peer_info.protocol_version = client_version;

    let server = serve_directly::<RoleServer, _, _, _, _>(
        ToolServer,
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

    let result = client
        .call_tool(CallToolRequestParams::new("echo"))
        .await
        .expect("tool call should succeed");

    client.cancel().await.expect("client should cancel");
    server_handle.await.expect("server task").expect("server");
    result.result_type
}

#[tokio::test]
async fn legacy_version_omits_result_type() {
    assert_eq!(
        call_tool_result_type(ProtocolVersion::V_2025_11_25).await,
        None
    );
}

#[tokio::test]
async fn sep_2322_version_gets_complete_result_type() {
    assert_eq!(
        call_tool_result_type(ProtocolVersion::V_2026_07_28).await,
        Some(ResultType::COMPLETE),
    );
}
