// Example of using SSE server transport with actix-web framework
// This requires the "actix-web" feature to be enabled in Cargo.toml
use rmcp::transport::sse_server::{SseServer, SseServerConfig};
use tracing_subscriber::{
    layer::SubscriberExt,
    util::SubscriberInitExt,
    {self},
};
mod common;
use common::counter::Counter;

const BIND_ADDRESS: &str = "127.0.0.1:8000";

// Note: Using #[actix_web::main] instead of #[tokio::main]
// This sets up the actix-web runtime which is required for actix-web transports
#[actix_web::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "debug".to_string().into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let config = SseServerConfig {
        bind: BIND_ADDRESS.parse()?,
        sse_path: "/sse".to_string(),
        post_path: "/message".to_string(),
        ct: tokio_util::sync::CancellationToken::new(),
        sse_keep_alive: None,
    };

    let ct_signal = config.ct.clone();

    // When actix-web feature is enabled, SseServer uses actix-web implementation
    // The same API works with both axum and actix-web
    let sse_server = SseServer::serve_with_config(config).await?;
    let bind_addr = sse_server.config.bind;
    let ct = sse_server.with_service(Counter::new);

    println!(
        "\n🚀 SSE Server (actix-web) running at http://{}",
        bind_addr
    );
    println!("📡 SSE endpoint: http://{}/sse", bind_addr);
    println!("📮 Message endpoint: http://{}/message", bind_addr);
    println!("\nPress Ctrl+C to stop the server\n");

    // Set up Ctrl-C handler
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        println!("\n⏹️  Shutting down...");
        ct_signal.cancel();
    });

    // Wait for cancellation
    ct.cancelled().await;
    println!("✅ Server stopped");
    Ok(())
}
