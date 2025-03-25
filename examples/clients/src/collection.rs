use std::collections::HashMap;

use anyhow::Result;
use rmcp::service::ServiceExt;
use rmcp::{model::CallToolRequestParam, transport::TokioChildProcess};

use tokio::process::Command;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

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

    let mut client_list = HashMap::new();
    for idx in 0..10 {
        let service = ()
            .into_dyn()
            .serve(TokioChildProcess::new(
                Command::new("uvx").arg("mcp-server-git"),
            )?)
            .await?;
        client_list.insert(idx, service);
    }

    for (_, service) in client_list.iter() {
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
    for (_, service) in client_list {
        service.cancel().await?;
    }
    Ok(())
}
