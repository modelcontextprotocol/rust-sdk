use std::env;

use rmcp::{
    ServiceExt,
    transport::{
        stdio,
        streamable_http_server::{StreamableHttpService, session::local::LocalSessionManager},
    },
};

mod common;
use common::progress_demo::ProgressDemo;

const HTTP_BIND_ADDRESS: &str = "127.0.0.1:8001";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Get transport mode from environment variable or command line argument
    let transport_mode = env::args()
        .nth(1)
        .unwrap_or_else(|| env::var("TRANSPORT_MODE").unwrap_or_else(|_| "stdio".to_string()));

    match transport_mode.as_str() {
        "stdio" => run_stdio().await,
        "http" | "streamhttp" => run_streamable_http().await,
        "all" => run_all_transports().await,
        _ => {
            eprintln!("Usage: {} [stdio|http|all]", env::args().next().unwrap());
            std::process::exit(1);
        }
    }
}

async fn run_stdio() -> anyhow::Result<()> {
    let server = ProgressDemo::new();
    let service = server.serve(stdio()).await.inspect_err(|e| {
        tracing::error!("stdio serving error: {:?}", e);
    })?;

    service.waiting().await?;
    Ok(())
}

async fn run_streamable_http() -> anyhow::Result<()> {
    println!("Running Streamable HTTP server");
    let service = StreamableHttpService::new(
        || Ok(ProgressDemo::new()),
        LocalSessionManager::default().into(),
        Default::default(),
    );

    let router = axum::Router::new().nest_service("/mcp", service);
    let tcp_listener = tokio::net::TcpListener::bind(HTTP_BIND_ADDRESS).await?;

    tracing::info!(
        "Progress Demo HTTP server started at http://{}/mcp",
        HTTP_BIND_ADDRESS
    );
    tracing::info!("Press Ctrl+C to shutdown");

    let _ = axum::serve(tcp_listener, router)
        .with_graceful_shutdown(async { tokio::signal::ctrl_c().await.unwrap() })
        .await;

    Ok(())
}

async fn run_all_transports() -> anyhow::Result<()> {
    println!("Running all transports");

    // Start Streamable HTTP server
    let http_service = StreamableHttpService::new(
        || Ok(ProgressDemo::new()),
        LocalSessionManager::default().into(),
        Default::default(),
    );
    let http_router = axum::Router::new().nest_service("/mcp", http_service);
    let http_listener = tokio::net::TcpListener::bind(HTTP_BIND_ADDRESS).await?;

    // Start Streamable HTTP server
    tokio::spawn(async move {
        let _ = axum::serve(http_listener, http_router)
            .with_graceful_shutdown(async { tokio::signal::ctrl_c().await.unwrap() })
            .await;
    });

    tracing::info!(
        "Progress Demo HTTP server started at http://{}/mcp",
        HTTP_BIND_ADDRESS
    );
    tracing::info!("Press Ctrl+C to shutdown");

    tokio::signal::ctrl_c().await?;

    Ok(())
}
