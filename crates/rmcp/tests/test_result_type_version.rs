//! SEP-2322: the `resultType` discriminator follows the negotiated protocol version.
//!
//! Peers negotiating `2026-07-28` or newer receive `resultType: "complete"` on
//! ordinary results; older peers keep the legacy wire shape without the field.
#![cfg(not(feature = "local"))]
#![cfg(feature = "client")]

use rmcp::{
    ClientHandler, RoleServer, ServerHandler, ServiceExt,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ErrorData,
        InitializeRequestParams, ProtocolVersion, ResultType,
    },
    service::RequestContext,
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
    fn get_info(&self) -> InitializeRequestParams {
        let mut info = InitializeRequestParams::default();
        info.protocol_version = self.protocol_version.clone();
        info
    }
}

async fn call_tool_result_type(client_version: ProtocolVersion) -> Option<ResultType> {
    let (server_transport, client_transport) = tokio::io::duplex(4096);

    let server_handle = tokio::spawn(async move {
        ToolServer.serve(server_transport).await?.waiting().await?;
        anyhow::Ok(())
    });

    let client = VersionedClient {
        protocol_version: client_version,
    }
    .serve(client_transport)
    .await
    .expect("client should connect");

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
