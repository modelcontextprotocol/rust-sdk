use anyhow::Result;
use rmcp::{
    ClientLifecycleMode, ClientServiceExt,
    model::{
        CallToolRequestParams, ClientCapabilities, ClientInfo, Implementation, ProtocolVersion,
    },
    transport::StreamableHttpClientTransport,
};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| format!("info,{}=debug", env!("CARGO_CRATE_NAME")).into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();
    let transport = StreamableHttpClientTransport::from_uri("http://localhost:8000/mcp");
    let client_info = ClientInfo::new(
        ClientCapabilities::default(),
        Implementation::new("streamable-http-client", "0.0.1"),
    );
    let client = client_info
        .serve_with_lifecycle(
            transport,
            ClientLifecycleMode::Auto {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
                legacy_version: Some(ProtocolVersion::V_2025_11_25),
            },
        )
        .await
        .inspect_err(|e| {
            tracing::error!("client error: {:?}", e);
        })?;

    let server_info = client.peer_info();
    tracing::info!("Connected to server: {server_info:#?}");

    let tools = client.list_tools(Default::default()).await?;
    tracing::info!("Available tools: {tools:#?}");

    let tool_result = client
        .call_tool(CallToolRequestParams::new("increment"))
        .await?;
    tracing::info!("Tool result: {tool_result:#?}");
    client.cancel().await?;
    Ok(())
}
