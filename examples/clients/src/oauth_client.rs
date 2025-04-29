use std::{net::SocketAddr, sync::Arc, time::Duration};

use anyhow::{Context, Result};
use axum::{
    Router,
    extract::{Query, State},
    response::Html,
    routing::get,
};
use rmcp::{
    ServiceExt,
    model::ClientInfo,
    transport::{auth::OAuthState, create_authorized_transport, sse::SseTransportRetryConfig},
};
use serde::Deserialize;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter},
    sync::{Mutex, oneshot},
};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

const MCP_SERVER_URL: &str = "http://localhost:3000/mcp";
const MCP_REDIRECT_URI: &str = "http://localhost:8080/callback";
const MCP_SSE_URL: &str = "http://localhost:3000/mcp/sse";
const CALLBACK_PORT: u16 = 8080;
const CALLBACK_HTML: &str = include_str!("callback.html");

#[derive(Clone)]
struct AppState {
    code_receiver: Arc<Mutex<Option<oneshot::Sender<String>>>>,
}

#[derive(Debug, Deserialize)]
struct CallbackParams {
    code: String,
    #[allow(dead_code)]
    state: Option<String>,
}

async fn callback_handler(
    Query(params): Query<CallbackParams>,
    State(state): State<AppState>,
) -> Html<String> {
    tracing::info!("Received callback with code: {}", params.code);

    // Send the code to the main thread
    if let Some(sender) = state.code_receiver.lock().await.take() {
        let _ = sender.send(params.code);
    }
    // Return success page
    Html(CALLBACK_HTML.to_string())
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "debug".to_string().into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();
    // it is a http server for handling callback
    // Create channel for receiving authorization code
    let (code_sender, code_receiver) = oneshot::channel::<String>();

    // Create app state
    let app_state = AppState {
        code_receiver: Arc::new(Mutex::new(Some(code_sender))),
    };

    // Start HTTP server for handling callbacks
    let app = Router::new()
        .route("/callback", get(callback_handler))
        .with_state(app_state);

    let addr = SocketAddr::from(([127, 0, 0, 1], CALLBACK_PORT));
    tracing::info!("Starting callback server at: http://{}", addr);

    // Start server in a separate task
    tokio::spawn(async move {
        let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
        let result = axum::serve(listener, app).await;

        if let Err(e) = result {
            tracing::error!("Callback server error: {}", e);
        }
    });

    // Get server URL
    let server_url = MCP_SERVER_URL.to_string();
    tracing::info!("Using MCP server URL: {}", server_url);

    // Configure retry settings
    let retry_config = SseTransportRetryConfig {
        max_times: Some(3),
        min_duration: Duration::from_secs(1),
    };
    // Initialize oauth state machine
    let mut oauth_state = OAuthState::new(&server_url, None)
        .await
        .context("Failed to initialize oauth state machine")?;
    oauth_state
        .start_authorization(&["mcp", "profile", "email"], MCP_REDIRECT_URI)
        .await
        .context("Failed to start authorization")?;

    // Output authorization URL to user
    let mut output = BufWriter::new(tokio::io::stdout());
    output.write_all(b"\n=== MCP OAuth Client ===\n\n").await?;
    output
        .write_all(b"Please open the following URL in your browser to authorize:\n\n")
        .await?;
    output
        .write_all(oauth_state.get_authorization_url().await?.as_bytes())
        .await?;
    output
        .write_all(b"\n\nWaiting for browser callback, please do not close this window...\n")
        .await?;
    output.flush().await?;

    // Wait for authorization code
    tracing::info!("Waiting for authorization code...");
    let auth_code = code_receiver
        .await
        .context("Failed to get authorization code")?;
    tracing::info!("Received authorization code: {}", auth_code);
    // Exchange code for access token
    tracing::info!("Exchanging authorization code for access token...");
    oauth_state
        .handle_callback(&auth_code)
        .await
        .context("Failed to handle callback")?;
    tracing::info!("Successfully obtained access token");

    output
        .write_all(b"\nAuthorization successful! Access token obtained.\n\n")
        .await?;
    output.flush().await?;

    // Create authorized transport, this transport is authorized by the oauth state machine
    tracing::info!("Establishing authorized connection to MCP server...");
    let transport =
        match create_authorized_transport(MCP_SSE_URL.to_string(), oauth_state, Some(retry_config))
            .await
        {
            Ok(t) => t,
            Err(e) => {
                tracing::error!("Failed to create authorized transport: {}", e);
                return Err(anyhow::anyhow!("Connection failed: {}", e));
            }
        };

    // Create client and connect to MCP server
    let client_service = ClientInfo::default();
    let client = client_service.serve(transport).await?;
    tracing::info!("Successfully connected to MCP server");

    // Test API requests
    output
        .write_all(b"Fetching available tools from server...\n")
        .await?;
    output.flush().await?;

    match client.peer().list_all_tools().await {
        Ok(tools) => {
            output
                .write_all(format!("Available tools: {}\n\n", tools.len()).as_bytes())
                .await?;
            for tool in tools {
                output
                    .write_all(
                        format!(
                            "- {} ({})\n",
                            tool.name,
                            tool.description.unwrap_or_default()
                        )
                        .as_bytes(),
                    )
                    .await?;
            }
        }
        Err(e) => {
            output
                .write_all(format!("Error fetching tools: {}\n", e).as_bytes())
                .await?;
        }
    }

    output
        .write_all(b"\nFetching available prompts from server...\n")
        .await?;
    output.flush().await?;

    match client.peer().list_all_prompts().await {
        Ok(prompts) => {
            output
                .write_all(format!("Available prompts: {}\n\n", prompts.len()).as_bytes())
                .await?;
            for prompt in prompts {
                output
                    .write_all(format!("- {}\n", prompt.name).as_bytes())
                    .await?;
            }
        }
        Err(e) => {
            output
                .write_all(format!("Error fetching prompts: {}\n", e).as_bytes())
                .await?;
        }
    }

    output
        .write_all(b"\nConnection established successfully. You are now authenticated with the MCP server.\n")
        .await?;
    output.flush().await?;

    // Keep the program running, wait for user input to exit
    output.write_all(b"\nPress Enter to exit...\n").await?;
    output.flush().await?;

    let mut input = String::new();
    let mut reader = BufReader::new(tokio::io::stdin());
    reader.read_line(&mut input).await?;

    Ok(())
}
