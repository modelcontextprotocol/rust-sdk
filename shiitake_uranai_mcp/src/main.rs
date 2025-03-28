use anyhow::Result;
use mcp_server::router::RouterService;
use mcp_server::{ByteTransport, Server};
use shiitake_domain::constellation_validator::validate_constellation;
use std::env;
use tokio::io::{stdin, stdout};
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::{self, EnvFilter};

mod server;
mod shiitake_domain;

#[tokio::main]
async fn main() -> Result<()> {
    // Set up file appender for logging
    let file_appender = RollingFileAppender::new(Rotation::DAILY, "logs", "mcp-server.log");

    // Initialize the tracing subscriber with file and stdout logging
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive(tracing::Level::INFO.into()))
        .with_writer(file_appender)
        .with_target(false)
        .with_thread_ids(true)
        .with_file(true)
        .with_line_number(true)
        .init();

    tracing::info!("Starting MCP server");

    let constellation = env::var("CONSTELLATION").map_err(|_| {
        tracing::error!("CONSTELLATION environment variable not set");
        anyhow::anyhow!("CONSTELLATION environment variable is required")
    })?;

    match validate_constellation(&constellation) {
        true => tracing::info!("Using constellation: {}", constellation),
        false => {
            tracing::error!("Invalid constellation: {}", constellation);
            return Err(anyhow::anyhow!("CONSTELLATION must be one of: aries, taurus, gemini, cancer, leo, virgo, libra, scorpio, sagittarius, capricorn, aquarius, pisces"));
        }
    }

    // Create an instance of our counter router
    let router = RouterService(
        server::shiitake_uranai_mcp_server::ShiitakeUranaiRouter::new(constellation.to_string()),
    );

    // Create and run the server
    let server = Server::new(router);
    let transport = ByteTransport::new(stdin(), stdout());

    tracing::info!("Server initialized and ready to handle requests");
    Ok(server.run(transport).await?)
}
