//! SEP-2106: non-object tool output follows the negotiated protocol version.
//!
//! Peers negotiating `2026-07-28` or newer receive any JSON Schema as a tool's
//! `outputSchema` and any JSON value as `structuredContent`. Older versions
//! require both to be objects, so non-object ones are omitted and the result
//! reaches the peer through its text content.
#![cfg(not(feature = "local"))]
#![cfg(feature = "client")]

use rmcp::{
    ClientHandler, Json, RoleClient, RoleServer, ServerHandler,
    handler::server::router::tool::ToolRouter,
    model::{CallToolRequestParams, ClientConfig, InitializeResult, ProtocolVersion, Tool},
    service::{RunningService, serve_directly},
    tool, tool_handler, tool_router,
};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::json;
use tokio::task::JoinHandle;

const LEGACY_VERSIONS: &[ProtocolVersion] =
    &[ProtocolVersion::V_2025_06_18, ProtocolVersion::V_2025_11_25];

#[derive(Serialize, JsonSchema)]
struct Total {
    sum: i32,
}

#[derive(Debug, Clone)]
struct OutputServer {
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl OutputServer {
    fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }

    #[tool(description = "Returns a list of names")]
    async fn names(&self) -> Json<Vec<String>> {
        Json(vec!["a".to_string(), "b".to_string()])
    }

    #[tool(description = "Returns an object")]
    async fn total(&self) -> Json<Total> {
        Json(Total { sum: 3 })
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for OutputServer {}

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
fn connect(
    protocol_version: ProtocolVersion,
) -> (RunningService<RoleClient, VersionedClient>, ServerTask) {
    let (server_transport, client_transport) = tokio::io::duplex(4096);

    let client_handler = VersionedClient {
        protocol_version: protocol_version.clone(),
    };
    let mut server_peer_info = InitializeResult::default();
    server_peer_info.protocol_version = protocol_version;

    let server = serve_directly::<RoleServer, _, _, _, _>(
        OutputServer::new(),
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

fn tool_named<'a>(tools: &'a [Tool], name: &str) -> &'a Tool {
    tools
        .iter()
        .find(|tool| tool.name == name)
        .expect("tool should be listed")
}

#[tokio::test]
async fn legacy_versions_omit_non_object_output_schema() {
    for version in LEGACY_VERSIONS {
        let (client, server_task) = connect(version.clone());
        let tools = client.list_tools(None).await.expect("tools/list").tools;
        disconnect(client, server_task).await;

        assert_eq!(
            tool_named(&tools, "names").output_schema,
            None,
            "{version} requires an object outputSchema"
        );
        let total_schema = tool_named(&tools, "total").output_schema.as_ref();
        assert_eq!(
            total_schema.and_then(|schema| schema.get("type")),
            Some(&json!("object")),
            "{version} keeps object outputSchema"
        );
    }
}

#[tokio::test]
async fn legacy_versions_omit_non_object_structured_content() {
    for version in LEGACY_VERSIONS {
        let (client, server_task) = connect(version.clone());
        let names = client
            .call_tool(CallToolRequestParams::new("names"))
            .await
            .expect("names call should succeed");
        let total = client
            .call_tool(CallToolRequestParams::new("total"))
            .await
            .expect("total call should succeed");
        disconnect(client, server_task).await;

        assert_eq!(
            names.structured_content, None,
            "{version} requires object structuredContent"
        );
        assert_eq!(
            names
                .into_typed::<Vec<String>>()
                .expect("text content should carry the result"),
            ["a", "b"],
        );
        assert_eq!(
            total.structured_content,
            Some(json!({ "sum": 3 })),
            "{version} keeps object structuredContent"
        );
    }
}

#[tokio::test]
async fn sep_2106_version_keeps_non_object_output() {
    let (client, server_task) = connect(ProtocolVersion::V_2026_07_28);
    let tools = client.list_tools(None).await.expect("tools/list").tools;
    let names = client
        .call_tool(CallToolRequestParams::new("names"))
        .await
        .expect("names call should succeed");
    disconnect(client, server_task).await;

    let names_schema = tool_named(&tools, "names").output_schema.as_ref();
    assert_eq!(
        names_schema.and_then(|schema| schema.get("type")),
        Some(&json!("array"))
    );
    assert_eq!(names.structured_content, Some(json!(["a", "b"])));
}
