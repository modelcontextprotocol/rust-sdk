use anyhow::Result;
use common::counter::Counter;
use rmcp::{ServiceExt, transport::stdio};
use tracing_subscriber::{self, EnvFilter};
mod common;
/// From the repository root:
/// ```sh
/// cargo build -p mcp-server-examples --example servers_counter_stdio
/// npx @modelcontextprotocol/inspector --cli ./target/debug/examples/servers_counter_stdio --method tools/list
/// ```
#[tokio::main]
async fn main() -> Result<()> {
    // Write logs to stderr; stdout carries MCP protocol messages.
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive(tracing::Level::DEBUG.into()))
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    tracing::info!("Starting MCP server");

    // The MCP client launches this process and writes protocol messages to stdin.
    // This await waits for startup input (e.g. initialize for a legacy client).
    let service = Counter::new().serve(stdio()).await.inspect_err(|e| {
        tracing::error!("serving error: {:?}", e);
    })?;

    // Wait for the running service to stop after startup has completed.
    service.waiting().await?;
    Ok(())
}
