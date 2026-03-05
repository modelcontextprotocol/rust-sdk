use std::env;

use anyhow::{Context, Result};
use rmcp::{
    ServiceExt,
    model::ClientInfo,
    transport::{
        StreamableHttpClientTransport,
        auth::{AuthClient, ClientCredentialsConfig, OAuthState},
        streamable_http_client::StreamableHttpClientTransportConfig,
    },
};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

/// Example: OAuth 2.0 Client Credentials flow (SEP-1046)
///
/// Usage:
///   cargo run -p mcp-client-examples --example clients_client_credentials -- <server_url> <client_id> <client_secret>
#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".to_string().into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let args: Vec<String> = env::args().collect();
    let server_url = args
        .get(1)
        .context("Usage: <server_url> <client_id> <client_secret>")?
        .clone();
    let client_id = args
        .get(2)
        .context("Usage: <server_url> <client_id> <client_secret>")?
        .clone();
    let client_secret = args
        .get(3)
        .context("Usage: <server_url> <client_id> <client_secret>")?
        .clone();

    tracing::info!("Connecting to MCP server at: {}", server_url);
    tracing::info!("Using client_id: {}", client_id);

    // Initialize OAuth state and authenticate with client credentials
    let mut oauth_state = OAuthState::new(&server_url, None)
        .await
        .context("Failed to initialize OAuth state")?;

    let config = ClientCredentialsConfig::ClientSecret {
        client_id,
        client_secret,
        scopes: vec![],
        resource: Some(server_url.clone()),
    };

    oauth_state
        .authenticate_client_credentials(config)
        .await
        .context("Client credentials authentication failed")?;

    tracing::info!("Successfully authenticated with client credentials");

    // Create authorized transport
    let manager = oauth_state
        .into_authorization_manager()
        .context("Failed to get authorization manager")?;
    let client = AuthClient::new(reqwest::Client::default(), manager);
    let transport = StreamableHttpClientTransport::with_client(
        client,
        StreamableHttpClientTransportConfig::with_uri(server_url.as_str()),
    );

    // Connect to MCP server and list tools
    let client_service = ClientInfo::default();
    let client = client_service.serve(transport).await?;
    tracing::info!("Connected to MCP server");

    match client.peer().list_all_tools().await {
        Ok(tools) => {
            println!("Available tools ({}):", tools.len());
            for tool in tools {
                println!(
                    "  - {} ({})",
                    tool.name,
                    tool.description.unwrap_or_default()
                );
            }
        }
        Err(e) => {
            tracing::error!("Failed to list tools: {}", e);
        }
    }

    Ok(())
}
