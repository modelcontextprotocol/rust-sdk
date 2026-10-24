#![cfg(not(feature = "local"))]
#![cfg(feature = "client")]

use rmcp::{
    ClientHandler, ErrorData, RoleClient, RoleServer, ServerHandler,
    handler::server::router::{prompt::PromptRouter, tool::ToolRouter},
    model::{
        CacheScope, ClientConfig, InitializeResult, ListPromptsResult, ListResourcesResult,
        ListToolsResult, PaginatedRequestParams, ProtocolVersion, ReadResourceRequestParams,
        ReadResourceResponse, ReadResourceResult, ResourceContents,
    },
    prompt_handler,
    service::{RequestContext, RunningService, serve_directly},
    tool_handler,
};
use tokio::task::JoinHandle;

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

/// Implements its handlers by hand instead of through the macros, leaving the
/// caching hints unset.
#[derive(Debug, Clone)]
struct ManualServer;

impl ServerHandler for ManualServer {
    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(vec![]))
    }

    async fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, ErrorData> {
        Ok(ListPromptsResult::with_all_items(vec![]))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        Ok(ReadResourceResult::new(vec![ResourceContents::text("hello", request.uri)]).into())
    }
}

/// Sets its own caching hints, which the server must pass through unchanged.
#[derive(Debug, Clone)]
struct ExplicitHintServer;

impl ServerHandler for ExplicitHintServer {
    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        Ok(ListResourcesResult::default()
            .with_ttl_ms(5_000)
            .with_cache_scope(CacheScope::Public))
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

type ServerTask = JoinHandle<anyhow::Result<()>>;

/// Wires the pair up directly on `protocol_version`. `2026-07-28` removed the
/// `initialize` handshake, so a peer on that revision is reached the way the
/// discover lifecycle leaves one: with the version already agreed.
fn connect<S: ServerHandler>(
    server: S,
    protocol_version: ProtocolVersion,
) -> (RunningService<RoleClient, VersionedClient>, ServerTask) {
    let (server_transport, client_transport) = tokio::io::duplex(4096);

    let client_handler = VersionedClient {
        protocol_version: protocol_version.clone(),
    };
    let mut server_peer_info = InitializeResult::default();
    server_peer_info.protocol_version = protocol_version;

    let server = serve_directly::<RoleServer, _, _, _, _>(
        server,
        server_transport,
        Some(client_handler.get_info()),
    );
    let server_task = tokio::spawn(async move {
        server.waiting().await?;
        anyhow::Ok(())
    });

    let client = serve_directly::<RoleClient, _, _, _, _>(
        client_handler,
        client_transport,
        Some(server_peer_info.into()),
    );
    (client, server_task)
}

async fn disconnect(client: RunningService<RoleClient, VersionedClient>, server_task: ServerTask) {
    client.cancel().await.expect("client should cancel");
    server_task.await.expect("server task").expect("server");
}

#[tokio::test]
async fn handler_macros_should_emit_required_cache_hints_for_2026_07_28() {
    let (client, server_task) = connect(CacheHintServer::new(), ProtocolVersion::V_2026_07_28);

    let tools = client.list_tools(None).await.expect("tools/list");
    let prompts = client.list_prompts(None).await.expect("prompts/list");
    disconnect(client, server_task).await;

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
    let (client, server_task) = connect(CacheHintServer::new(), ProtocolVersion::V_2025_11_25);

    let tools = client.list_tools(None).await.expect("tools/list");
    let prompts = client.list_prompts(None).await.expect("prompts/list");
    disconnect(client, server_task).await;

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

#[tokio::test]
async fn manual_list_tools_and_prompts_should_get_default_cache_hints_for_2026_07_28() {
    let (client, server_task) = connect(ManualServer, ProtocolVersion::V_2026_07_28);

    let tools = client.list_tools(None).await.expect("tools/list");
    let prompts = client.list_prompts(None).await.expect("prompts/list");
    disconnect(client, server_task).await;

    assert_eq!(
        (
            tools.ttl_ms,
            tools.cache_scope,
            prompts.ttl_ms,
            prompts.cache_scope,
        ),
        (
            Some(0),
            Some(CacheScope::Private),
            Some(0),
            Some(CacheScope::Private),
        )
    );
}

#[tokio::test]
async fn list_resources_should_get_default_cache_hints_for_2026_07_28() {
    let (client, server_task) = connect(ManualServer, ProtocolVersion::V_2026_07_28);

    let resources = client.list_resources(None).await.expect("resources/list");
    disconnect(client, server_task).await;

    assert_eq!(
        (resources.ttl_ms, resources.cache_scope),
        (Some(0), Some(CacheScope::Private))
    );
}

#[tokio::test]
async fn list_resource_templates_should_get_default_cache_hints_for_2026_07_28() {
    let (client, server_task) = connect(ManualServer, ProtocolVersion::V_2026_07_28);

    let templates = client
        .list_resource_templates(None)
        .await
        .expect("resources/templates/list");
    disconnect(client, server_task).await;

    assert_eq!(
        (templates.ttl_ms, templates.cache_scope),
        (Some(0), Some(CacheScope::Private))
    );
}

#[tokio::test]
async fn read_resource_should_get_default_cache_hints_for_2026_07_28() {
    let (client, server_task) = connect(ManualServer, ProtocolVersion::V_2026_07_28);

    let resource = client
        .read_resource(ReadResourceRequestParams::new("memo://hello"))
        .await
        .expect("resources/read");
    disconnect(client, server_task).await;

    assert_eq!(
        (resource.ttl_ms, resource.cache_scope),
        (Some(0), Some(CacheScope::Private))
    );
}

#[tokio::test]
async fn manual_results_should_omit_cache_hints_for_legacy_versions() {
    let (client, server_task) = connect(ManualServer, ProtocolVersion::V_2025_11_25);

    let tools = client.list_tools(None).await.expect("tools/list");
    let prompts = client.list_prompts(None).await.expect("prompts/list");
    let resources = client.list_resources(None).await.expect("resources/list");
    let resource = client
        .read_resource(ReadResourceRequestParams::new("memo://hello"))
        .await
        .expect("resources/read");
    disconnect(client, server_task).await;

    assert_eq!(
        (
            tools.ttl_ms,
            tools.cache_scope,
            prompts.ttl_ms,
            prompts.cache_scope,
            resources.ttl_ms,
            resources.cache_scope,
            resource.ttl_ms,
            resource.cache_scope,
        ),
        (None, None, None, None, None, None, None, None)
    );
}

#[tokio::test]
async fn explicit_cache_hints_should_be_kept_for_2026_07_28() {
    let (client, server_task) = connect(ExplicitHintServer, ProtocolVersion::V_2026_07_28);

    let resources = client.list_resources(None).await.expect("resources/list");
    disconnect(client, server_task).await;

    assert_eq!(
        (resources.ttl_ms, resources.cache_scope),
        (Some(5_000), Some(CacheScope::Public))
    );
}
