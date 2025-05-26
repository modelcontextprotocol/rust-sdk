/// This example show how to store multiple clients in a map and call tools on them.
/// into_dyn() is used to convert the service to a dynamic service.
/// For example, you can use this to call tools on a service that is running in a different process.
/// or a service that is running in a different machine.
use std::collections::HashMap;

use anyhow::Result;
use rmcp::{
    model::CallToolRequestParam,
    service::ServiceExt,
    transport::{ConfigureCommandExt, TokioChildProcess},
};
use tokio::process::Command;
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
        let service = ()
            .into_dyn()
            .serve(TokioChildProcess::new(Command::new("uvx").configure(
                |cmd| {
                    cmd.arg("mcp-client-git");
                },
            ))?)
            .await?;
        clients_map.insert(idx, service);
    }

    for (_, service) in clients_map.iter() {
        // Initialize
        let _server_info = service.peer_info();

        // List tools
        let _tools = service.list_tools(Default::default()).await?;

        // Call tool 'git_status' with arguments = {"repo_path": "."}
        let _tool_result = service
            .call_tool(CallToolRequestParam {
                name: "git_status".into(),
                arguments: serde_json::json!({ "repo_path": "." }).as_object().cloned(),
            })
            .await?;
    }
    for (_, service) in clients_map {
        service.cancel().await?;
    }
    Ok(())
}
