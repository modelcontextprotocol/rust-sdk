use rmcp::{
    RmcpError,
    model::CallToolRequestParams,
    service::ServiceExt,
    transport::{
        CommandBuilder,
        child_process::{tokio::TokioChildProcessRunner, transport::ChildProcessTransport},
    },
};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[allow(clippy::result_large_err)]
#[tokio::main]
async fn main() -> Result<(), RmcpError> {
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
        .spawn_dyn()
        .map_err(RmcpError::transport_creation::<TokioChildProcessRunner>)?;

    let transport = ChildProcessTransport::new(command)
        .map_err(RmcpError::transport_creation::<TokioChildProcessRunner>)?;

    let (client, work) = ().serve(transport).await?;
    tokio::spawn(work);

    // Initialize
    let server_info = client.peer_info();
    tracing::info!("Connected to server: {server_info:#?}");

    // List tools
    let tools = client.list_tools(Default::default()).await?;
    tracing::info!("Available tools: {tools:#?}");

    // Call tool 'git_status' with arguments = {"repo_path": "."}
    let tool_result = client
        .call_tool(CallToolRequestParams {
            meta: None,
            name: "git_status".into(),
            arguments: serde_json::json!({ "repo_path": "." }).as_object().cloned(),
            task: None,
        })
        .await?;
    tracing::info!("Tool result: {tool_result:#?}");
    client.cancel().await;
    Ok(())
}
