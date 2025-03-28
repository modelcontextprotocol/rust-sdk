use anyhow::Result;
use rmcp::{
    ServiceExt,
    model::{CallToolRequestParam, GetPromptRequestParam, ReadResourceRequestParam},
    object,
    transport::TokioChildProcess,
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

    // Start server
    let service = ()
        .serve(TokioChildProcess::new(
            Command::new("npx")
                .arg("-y")
                .arg("@modelcontextprotocol/server-everything"),
        )?)
        .await?;

    // Initialize
    let server_info = service.peer_info();
    tracing::info!("Connected to server: {server_info:#?}");

    // List tools
    let tools = service.list_all_tools().await?;
    tracing::info!("Available tools: {tools:#?}");

    // Call tool echo
    let tool_result = service
        .call_tool(CallToolRequestParam {
            name: "echo".into(),
            arguments: Some(object!({ "message": "hi from rmcp" })),
        })
        .await?;
    tracing::info!("Tool result for echo: {tool_result:#?}");

    // Call tool longRunningOperation
    let tool_result = service
        .call_tool(CallToolRequestParam {
            name: "longRunningOperation".into(),
            arguments: Some(object!({ "duration": 3, "steps": 1 })),
        })
        .await?;
    tracing::info!("Tool result for longRunningOperation: {tool_result:#?}");

    // List resources
    let resources = service.list_all_resources().await?;
    tracing::info!("Available resources: {resources:#?}");

    // Read resource
    let resource = service
        .read_resource(ReadResourceRequestParam {
            uri: "test://static/resource/3".into(),
        })
        .await?;
    tracing::info!("Resource: {resource:#?}");

    // List prompts
    let prompts = service.list_all_prompts().await?;
    tracing::info!("Available prompts: {prompts:#?}");

    // Get simple prompt
    let prompt = service
        .get_prompt(GetPromptRequestParam {
            name: "simple_prompt".into(),
            arguments: None,
        })
        .await?;
    tracing::info!("Prompt - simple: {prompt:#?}");

    // Get complex prompt (returns text & image)
    let prompt = service
        .get_prompt(GetPromptRequestParam {
            name: "complex_prompt".into(),
            arguments: Some(object!({ "temperature": "0.5", "style": "formal" })),
        })
        .await?;
    tracing::info!("Prompt - complex: {prompt:#?}");

    // List resource templates
    let resource_templates = service.list_all_resource_templates().await?;
    tracing::info!("Available resource templates: {resource_templates:#?}");

    service.cancel().await?;

    Ok(())
}
