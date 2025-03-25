use anyhow::Result;
use rmcp::{model::CallToolRequestParam, service::ServiceExt, transport::TokioChildProcess};

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
    let service = ()
        .serve(TokioChildProcess::new(
            Command::new("uvx").arg("mcp-server-git"),
        )?)
        .await?;

    // or
    // serve_client(
    //     (),
    //     TokioChildProcess::new(Command::new("uvx").arg("mcp-server-git"))?,
    // )
    // .await?;

    // Initialize
    let server_info = service.peer_info();
    tracing::info!("Connected to server: {server_info:#?}");

    // List tools
    let tools = service.list_tools(Default::default()).await?;
    tracing::info!("Available tools: {tools:#?}");

    // Call tool 'git_status' with arguments = {"repo_path": "."}
    let tool_result = service
        .call_tool(CallToolRequestParam {
            name: "git_status".into(),
            arguments: serde_json::json!({ "repo_path": "." }).as_object().cloned(),
        })
        .await?;
    tracing::info!("Tool result: {tool_result:#?}");
    service.cancel().await?;
    Ok(())
}
