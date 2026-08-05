#![cfg(not(feature = "local"))]
#![cfg(feature = "client")]

use rmcp::{
    ClientHandler, ServerHandler, ServiceExt,
    handler::server::router::{prompt::PromptRouter, tool::ToolRouter},
    model::{CacheScope, ClientInfo, ListPromptsResult, ListToolsResult, ProtocolVersion},
    prompt_handler, tool_handler,
};

#[derive(Debug, Clone)]
struct CacheHintServer {
    tool_router: ToolRouter<Self>,
    prompt_router: PromptRouter<Self>,
}

impl CacheHintServer {
    fn new() -> Self {
        Self {
            tool_router: ToolRouter::new(),
            prompt_router: PromptRouter::new(),
        }
    }
}

#[tool_handler(router = self.tool_router)]
#[prompt_handler(router = self.prompt_router)]
impl ServerHandler for CacheHintServer {}

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

async fn list_results(protocol_version: ProtocolVersion) -> (ListToolsResult, ListPromptsResult) {
    let (server_transport, client_transport) = tokio::io::duplex(4096);

    let server_handle = tokio::spawn(async move {
        CacheHintServer::new()
            .serve(server_transport)
            .await?
            .waiting()
            .await?;
        anyhow::Ok(())
    });

    let client = VersionedClient { protocol_version }
        .serve(client_transport)
        .await
        .expect("client should connect");
    let tools = client
        .list_tools(None)
        .await
        .expect("tools/list should succeed");
    let prompts = client
        .list_prompts(None)
        .await
        .expect("prompts/list should succeed");

    client.cancel().await.expect("client should cancel");
    server_handle.await.expect("server task").expect("server");
    (tools, prompts)
}

#[tokio::test]
async fn handler_macros_should_emit_required_cache_hints_for_2026_07_28() {
    let (tools, prompts) = list_results(ProtocolVersion::V_2026_07_28).await;

    assert_eq!(
        (
            tools.ttl_ms,
            tools.cache_scope,
            prompts.ttl_ms,
            prompts.cache_scope,
        ),
        (
            Some(0),
            Some(CacheScope::Public),
            Some(0),
            Some(CacheScope::Public),
        )
    );
}

#[tokio::test]
async fn handler_macros_should_omit_cache_hints_for_legacy_versions() {
    let (tools, prompts) = list_results(ProtocolVersion::V_2025_11_25).await;

    assert_eq!(
        (
            tools.ttl_ms,
            tools.cache_scope,
            prompts.ttl_ms,
            prompts.cache_scope,
        ),
        (None, None, None, None)
    );
}
