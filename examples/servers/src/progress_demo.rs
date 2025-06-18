use std::env;

use rmcp::{
    ServiceExt,
    transport::{
        sse_server::{SseServer, SseServerConfig},
        stdio,
        streamable_http_server::{StreamableHttpService, session::local::LocalSessionManager},
    },
};

mod common;
use common::progress_demo::ProgressDemo;

const SSE_BIND_ADDRESS: &str = "127.0.0.1:8000";
const HTTP_BIND_ADDRESS: &str = "127.0.0.1:8001";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Get transport mode from environment variable or command line argument
    let transport_mode = env::args()
        .nth(1)
        .unwrap_or_else(|| env::var("TRANSPORT_MODE").unwrap_or_else(|_| "stdio".to_string()));

    match transport_mode.as_str() {
        "stdio" => run_stdio().await,
        "sse" => run_sse().await,
        "http" | "streamhttp" => run_streamable_http().await,
        "all" => run_all_transports().await,
        _ => {
            eprintln!(
                "Usage: {} [stdio|sse|http|all]",
                env::args().next().unwrap()
            );
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

async fn run_sse() -> anyhow::Result<()> {
    println!("Running SSE server");
    let config = SseServerConfig {
        bind: SSE_BIND_ADDRESS.parse()?,
        sse_path: "/sse".to_string(),
        post_path: "/message".to_string(),
        ct: tokio_util::sync::CancellationToken::new(),
        sse_keep_alive: None,
    };

    let (sse_server, router) = SseServer::new(config);

    // Start the HTTP server for SSE
    let listener = tokio::net::TcpListener::bind(sse_server.config.bind).await?;
    let ct = sse_server.config.ct.child_token();

    let server = axum::serve(listener, router).with_graceful_shutdown(async move {
        ct.cancelled().await;
        tracing::info!("SSE server cancelled");
    });

    tokio::spawn(async move {
        if let Err(e) = server.await {
            tracing::error!(error = %e, "SSE server shutdown with error");
        }
    });

    // Start the MCP service with SSE transport
    let ct = sse_server.with_service(ProgressDemo::new);

    tracing::info!(
        "Progress Demo SSE server started at http://{}/sse",
        SSE_BIND_ADDRESS
    );
    tracing::info!("Press Ctrl+C to shutdown");

    tokio::signal::ctrl_c().await?;
    ct.cancel();
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
    // Start SSE server
    let sse_config = SseServerConfig {
        bind: SSE_BIND_ADDRESS.parse()?,
        sse_path: "/sse".to_string(),
        post_path: "/message".to_string(),
        ct: tokio_util::sync::CancellationToken::new(),
        sse_keep_alive: None,
    };

    let (sse_server, sse_router) = SseServer::new(sse_config);
    let sse_listener = tokio::net::TcpListener::bind(sse_server.config.bind).await?;
    let sse_ct = sse_server.config.ct.child_token();

    // Start Streamable HTTP server
    let http_service = StreamableHttpService::new(
        || Ok(ProgressDemo::new()),
        LocalSessionManager::default().into(),
        Default::default(),
    );
    let http_router = axum::Router::new().nest_service("/mcp", http_service);
    let http_listener = tokio::net::TcpListener::bind(HTTP_BIND_ADDRESS).await?;

    // Start SSE HTTP server
    let sse_http_server =
        axum::serve(sse_listener, sse_router).with_graceful_shutdown(async move {
            sse_ct.cancelled().await;
            tracing::info!("SSE server cancelled");
        });

    tokio::spawn(async move {
        if let Err(e) = sse_http_server.await {
            tracing::error!(error = %e, "SSE server shutdown with error");
        }
    });

    // Start Streamable HTTP server
    tokio::spawn(async move {
        let _ = axum::serve(http_listener, http_router)
            .with_graceful_shutdown(async { tokio::signal::ctrl_c().await.unwrap() })
            .await;
    });

    // Start MCP service with SSE
    let mcp_sse_ct = sse_server.with_service(ProgressDemo::new);

    tokio::signal::ctrl_c().await?;
    mcp_sse_ct.cancel();

    Ok(())
}
