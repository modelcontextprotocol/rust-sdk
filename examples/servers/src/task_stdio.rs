use anyhow::Result;
use common::task_demo::TaskDemo;
use rmcp::{ServiceExt, transport::stdio};
use tracing_subscriber::{self, EnvFilter};
mod common;

/// Stdio server demonstrating task-based tool invocation.
///
/// Run a matching client with:
///   cargo run -p mcp-client-examples --example clients_task_stdio
#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive(tracing::Level::INFO.into()))
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    tracing::info!("Starting task-demo MCP server");

    let service = TaskDemo::new().serve(stdio()).await.inspect_err(|e| {
        tracing::error!("serving error: {e:?}");
    })?;

    service.waiting().await?;
    Ok(())
}
