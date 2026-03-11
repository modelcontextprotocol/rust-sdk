use anyhow::Result;
use rmcp::{
    ServiceExt,
    model::{CallToolRequestParams, ClientCapabilities, ClientInfo, Implementation},
    transport::StreamableHttpClientTransport,
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
    let (transport, http_work) =
        StreamableHttpClientTransport::from_uri("http://localhost:8000/mcp");
    tokio::spawn(http_work);

    let client_info = ClientInfo::new(
        ClientCapabilities::default(),
        Implementation::new("test sse client", "0.0.1"),
    );
    let (client, work) = client_info.serve(transport).await.inspect_err(|e| {
        tracing::error!("client error: {:?}", e);
    })?;

    tokio::spawn(work);

    // Initialize
    let server_info = client.peer_info();
    tracing::info!("Connected to server: {server_info:#?}");

    // List tools
    let tools = client.list_tools(Default::default()).await?;
    tracing::info!("Available tools: {tools:#?}");

    let tool_result = client
        .call_tool(
            CallToolRequestParams::new("increment")
                .with_arguments(serde_json::json!({}).as_object().cloned().unwrap()),
        )
        .await?;
    tracing::info!("Tool result: {tool_result:#?}");
    client.cancel().await;
    Ok(())
}
