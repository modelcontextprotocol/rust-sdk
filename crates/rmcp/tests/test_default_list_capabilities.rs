#![cfg(all(feature = "client", feature = "server"))]

use rmcp::{
    ClientHandler, RoleClient, RoleServer, ServerHandler,
    model::{
        ArgumentInfo, ClientConfig, CompleteRequestParams, ErrorCode, InitializeResult,
        ProtocolVersion, Reference, ServerCapabilities, ServerConfig,
    },
    service::{ServiceError, serve_directly},
};

#[derive(Debug, Clone)]
struct CapabilityServer(ServerCapabilities);

impl ServerHandler for CapabilityServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(self.0.clone())
    }
}

#[derive(Debug, Clone)]
struct ProbeClient;

impl ClientHandler for ProbeClient {
    fn get_info(&self) -> ClientConfig {
        let mut info = ClientConfig::default();
        info.protocol_version = ProtocolVersion::V_2026_07_28;
        info
    }
}

fn expect_result<T: std::fmt::Debug>(
    method: &str,
    result: Result<T, ServiceError>,
    advertised: bool,
) -> Option<T> {
    if advertised {
        Some(result.unwrap_or_else(|error| panic!("{method} should succeed: {error:?}")))
    } else {
        match result {
            Err(ServiceError::McpError(error)) => {
                assert_eq!(error.code, ErrorCode::METHOD_NOT_FOUND, "{method}");
            }
            other => panic!("{method} should return method not found: {other:?}"),
        }
        None
    }
}

async fn check_capabilities(capabilities: ServerCapabilities) {
    let prompts = capabilities.prompts.is_some();
    let completions = capabilities.completions.is_some();
    let resources = capabilities.resources.is_some();
    let tools = capabilities.tools.is_some();

    let (server_transport, client_transport) = tokio::io::duplex(4096);
    let server = serve_directly::<RoleServer, _, _, _, _>(
        CapabilityServer(capabilities),
        server_transport,
        Some(ProbeClient.get_info()),
    );
    let mut peer_info = InitializeResult::default();
    peer_info.protocol_version = ProtocolVersion::V_2026_07_28;
    let client = serve_directly::<RoleClient, _, _, _, _>(
        ProbeClient,
        client_transport,
        Some(peer_info.into()),
    );

    if let Some(result) = expect_result("prompts/list", client.list_prompts(None).await, prompts) {
        assert!(result.prompts.is_empty());
    }
    if let Some(result) = expect_result(
        "completion/complete",
        client
            .complete(CompleteRequestParams::new(
                Reference::for_prompt("unused"),
                ArgumentInfo::new("argument", "value"),
            ))
            .await,
        completions,
    ) {
        assert!(result.completion.values.is_empty());
    }
    if let Some(result) = expect_result(
        "resources/list",
        client.list_resources(None).await,
        resources,
    ) {
        assert!(result.resources.is_empty());
    }
    if let Some(result) = expect_result(
        "resources/templates/list",
        client.list_resource_templates(None).await,
        resources,
    ) {
        assert!(result.resource_templates.is_empty());
    }
    if let Some(result) = expect_result("tools/list", client.list_tools(None).await, tools) {
        assert!(result.tools.is_empty());
    }

    client.cancel().await.expect("stop client");
    server.cancel().await.expect("stop server");
}

#[tokio::test]
async fn default_list_methods_follow_advertised_capabilities() {
    // LocalSet also lets this test run with --all-features, where rmcp's
    // `local` feature uses spawn_local for its service tasks.
    tokio::task::LocalSet::new()
        .run_until(async {
            check_capabilities(ServerCapabilities::default()).await;
            check_capabilities(ServerCapabilities::builder().enable_tools().build()).await;
            check_capabilities(
                ServerCapabilities::builder()
                    .enable_tools()
                    .enable_resources()
                    .enable_prompts()
                    .enable_completions()
                    .build(),
            )
            .await;
        })
        .await;
}
