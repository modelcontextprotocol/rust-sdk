use anyhow::Result;
use rmcp::{
    ServiceExt,
    model::{CallToolRequestParams, GetPromptRequestParams, ReadResourceRequestParams},
    object,
    transport::{
        self, CommandBuilder,
        child_process::{tokio::TokioChildProcessRunner, transport::ChildProcessTransport},
    },
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

    let command = CommandBuilder::<TokioChildProcessRunner>::new("npx")
        .arg("-y")
        .arg("@modelcontextprotocol/server-everything")
        .spawn_dyn()?;

    let transport = ChildProcessTransport::new(command)?;

    let (client, work) = ().serve(transport).await?;
    tokio::spawn(work);

    // Initialize
    let server_info = client.peer_info();
    tracing::info!("Connected to server: {server_info:#?}");

    // List tools
    let tools = client.list_all_tools().await?;
    tracing::info!("Available tools: {tools:#?}");

    // Call tool echo
    let tool_result = client
        .call_tool(
            CallToolRequestParams::new("echo")
                .with_arguments(object!({ "message": "hi from rmcp" })),
        )
        .await?;
    tracing::info!("Tool result for echo: {tool_result:#?}");

    // Call tool longRunningOperation
    let tool_result = client
        .call_tool(
            CallToolRequestParams::new("longRunningOperation")
                .with_arguments(object!({ "duration": 3, "steps": 1 })),
        )
        .await?;
    tracing::info!("Tool result for longRunningOperation: {tool_result:#?}");

    // List resources
    let resources = client.list_all_resources().await?;
    tracing::info!("Available resources: {resources:#?}");

    // Read resource
    let resource = client
        .read_resource(ReadResourceRequestParams::new("test://static/resource/3"))
        .await?;
    tracing::info!("Resource: {resource:#?}");

    // List prompts
    let prompts = client.list_all_prompts().await?;
    tracing::info!("Available prompts: {prompts:#?}");

    // Get simple prompt
    let prompt = client
        .get_prompt(GetPromptRequestParams::new("simple_prompt"))
        .await?;
    tracing::info!("Prompt - simple: {prompt:#?}");

    // Get complex prompt (returns text & image)
    let prompt = client
        .get_prompt(
            GetPromptRequestParams::new("complex_prompt")
                .with_arguments(object!({ "temperature": "0.5", "style": "formal" })),
        )
        .await?;
    tracing::info!("Prompt - complex: {prompt:#?}");

    // List resource templates
    let resource_templates = client.list_all_resource_templates().await?;
    tracing::info!("Available resource templates: {resource_templates:#?}");

    client.cancel().await;

    Ok(())
}
