/// This example show how to store multiple clients in a map and call tools on them.
/// into_dyn() is used to convert the service to a dynamic service.
/// For example, you can use this to call tools on a service that is running in a different process.
/// or a service that is running in a different machine.
use std::collections::HashMap;

use anyhow::Result;
use rmcp::{
    model::CallToolRequestParams,
    service::ServiceExt,
    transport::{
        CommandBuilder,
        child_process::{tokio::TokioChildProcessRunner, transport::ChildProcessTransport},
    },
};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| format!("info,{}=debug", env!("CARGO_CRATE_NAME")).into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let mut clients_map = HashMap::new();
    for idx in 0..10 {
        let child_process = CommandBuilder::<TokioChildProcessRunner>::new("uvx")
            .arg("mcp-client-git")
            .spawn_dyn()?;
        let transport = ChildProcessTransport::new(child_process)?;

        let (client, work) = ().into_dyn().serve(transport).await?;
        tokio::spawn(work);
        clients_map.insert(idx, client);
    }

    for (_, client) in clients_map.iter() {
        // Initialize
        let _server_info = client.peer_info();

        // List tools
        let _tools = client.list_tools(Default::default()).await?;

        // Call tool 'git_status' with arguments = {"repo_path": "."}
        let _tool_result = client
            .call_tool(
                CallToolRequestParams::new("git_status").with_arguments(
                    serde_json::json!({ "repo_path": "." })
                        .as_object()
                        .unwrap()
                        .clone(),
                ),
            )
            .await?;
    }
    for (_, service) in clients_map {
        service.cancel().await;
    }
    Ok(())
}
