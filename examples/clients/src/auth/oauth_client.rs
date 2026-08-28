use std::{env, net::SocketAddr, sync::Arc, time::Duration};

use anyhow::{Context, Result};
use axum::{
    Router,
    extract::{Query, State},
    response::Html,
    routing::get,
};
use rmcp::{
    RoleClient, ServiceExt,
    model::InitializeRequestParams,
    service::RunningService,
    transport::{
        StreamableHttpClientTransport,
        auth::{AuthClient, AuthorizationRequest, OAuthState},
        streamable_http_client::StreamableHttpClientTransportConfig,
    },
};
use serde::Deserialize;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter},
    sync::{Mutex, oneshot},
};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

const MCP_SERVER_URL: &str = "http://127.0.0.1:3000/mcp";
const MCP_REDIRECT_URI: &str = "http://127.0.0.1:8080/callback";
const CALLBACK_PORT: u16 = 8080;
const CALLBACK_HTML: &str = include_str!("callback.html");
const CLIENT_METADATA_URL: &str = "https://raw.githubusercontent.com/modelcontextprotocol/rust-sdk/refs/heads/main/client-metadata.json";

#[derive(Clone)]
struct AppState {
    code_receiver: Arc<Mutex<Option<oneshot::Sender<CallbackParams>>>>,
}

#[derive(Debug, Deserialize)]
struct CallbackParams {
    code: String,
    state: String,
    iss: Option<String>,
}

async fn callback_handler(
    Query(params): Query<CallbackParams>,
    State(state): State<AppState>,
) -> Html<String> {
    tracing::info!("Received callback: {params:?}");

    // Send the code to the main thread
    if let Some(sender) = state.code_receiver.lock().await.take() {
        let _ = sender.send(params);
    }
    // Return success page
    Html(CALLBACK_HTML.to_string())
}

enum ConnectOutcome {
    /// The server accepted the unauthenticated connection.
    Connected(RunningService<RoleClient, InitializeRequestParams>),
    /// The server answered 401; authorize with this `WWW-Authenticate`
    /// challenge and reconnect.
    AuthRequired(String),
}

/// Attempt the real connection unauthenticated — the reactive discovery
/// trigger (matching the TypeScript and Python SDKs). The server's 401
/// challenge, not a probe, tells us whether and how to authorize.
async fn try_connect(http_client: reqwest::Client, server_url: &str) -> Result<ConnectOutcome> {
    let transport = StreamableHttpClientTransport::with_client(
        http_client,
        StreamableHttpClientTransportConfig::with_uri(server_url),
    );
    match InitializeRequestParams::default().serve(transport).await {
        Ok(client) => Ok(ConnectOutcome::Connected(client)),
        Err(error) => match error.auth_challenge() {
            Some(challenge) => Ok(ConnectOutcome::AuthRequired(challenge.to_string())),
            None => Err(error.into()),
        },
    }
}

/// Run the browser OAuth flow seeded by the server's challenge, then
/// reconnect with the authorized transport.
async fn authorize_and_connect(
    challenge: String,
    oauth_http_client: reqwest::Client,
    server_url: &str,
    client_metadata_url: &str,
    code_receiver: oneshot::Receiver<CallbackParams>,
    output: &mut BufWriter<tokio::io::Stdout>,
) -> Result<RunningService<RoleClient, InitializeRequestParams>> {
    tracing::info!("Server requires authorization: {challenge}");

    // initialize oauth state machine
    let mut oauth_state = OAuthState::new(server_url, Some(oauth_http_client))
        .await
        .context("Failed to initialize oauth state machine")?;
    // Seed discovery from the server's challenge, and use CIMD (SEP-991)
    // with client metadata URL. Passing no scopes lets the SDK auto-select
    // from the challenge's scope hint, Protected Resource Metadata, or AS
    // metadata.
    oauth_state
        .start_authorization(
            AuthorizationRequest::new(MCP_REDIRECT_URI)
                .with_client_name("Test MCP Client")
                .with_client_metadata_url(client_metadata_url)
                .with_challenge(challenge),
        )
        .await
        .context("Failed to start authorization")?;

    // Output authorization URL to user
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
    let CallbackParams {
        code: auth_code,
        state: csrf_token,
        iss,
    } = code_receiver
        .await
        .context("Failed to get authorization code")?;
    tracing::info!("Received authorization code: {}", auth_code);
    // Exchange code for access token
    tracing::info!("Exchanging authorization code for access token...");
    oauth_state
        .handle_callback_with_issuer(&auth_code, &csrf_token, iss.as_deref())
        .await
        .context("Failed to handle callback")?;
    tracing::info!("Successfully obtained access token");

    output
        .write_all(b"\nAuthorization successful! Access token obtained.\n\n")
        .await?;
    output.flush().await?;

    // Reconnect with the authorized transport
    tracing::info!("Establishing authorized connection to MCP server...");
    let am = oauth_state
        .into_authorization_manager()
        .ok_or_else(|| anyhow::anyhow!("Failed to get authorization manager"))?;
    let auth_client = AuthClient::new(reqwest::Client::default(), am);
    let transport = StreamableHttpClientTransport::with_client(
        auth_client,
        StreamableHttpClientTransportConfig::with_uri(server_url),
    );
    Ok(InitializeRequestParams::default().serve(transport).await?)
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
    let (code_sender, code_receiver) = oneshot::channel::<CallbackParams>();

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
    tracing::warn!(
        "Note: Callback server may not receive callbacks if redirect URI doesn't match localhost if using CIMD (SEP-991)"
    );

    // Start server in a separate task
    tokio::spawn(async move {
        let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
        let result = axum::serve(listener, app).await;

        if let Err(e) = result {
            tracing::error!("Callback server error: {}", e);
        }
    });

    // Get server URL and client metadata URL from CLI (with defaults)
    //
    // Usage:
    //   cargo run -p mcp-client-examples --example clients_oauth_client -- <server_url> <client_metadata_url>
    let args: Vec<String> = env::args().collect();
    let server_url = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| MCP_SERVER_URL.to_string());
    let client_metadata_url = args
        .get(2)
        .cloned()
        .unwrap_or_else(|| CLIENT_METADATA_URL.to_string());

    tracing::info!("Using MCP server URL: {}", server_url);
    tracing::info!(
        "Using CIMD (SEP-991) with client metadata URL: {}",
        client_metadata_url
    );

    // Configure the HTTP client used for OAuth discovery, registration, token
    // exchange, and refresh. Customize this builder for proxies, TLS roots,
    // default headers, or other reqwest settings required by your environment.
    let oauth_http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .context("Failed to build OAuth HTTP client")?;

    let mut output = BufWriter::new(tokio::io::stdout());
    output.write_all(b"\n=== MCP OAuth Client ===\n\n").await?;
    output.flush().await?;

    // Reactive discovery: attempt the real connection first. The server's
    // 401 challenge — not a probe — tells us whether and how to authorize.
    // The transport gets a default client: `oauth_http_client`'s request
    // timeout would cut long-lived SSE streams short.
    tracing::info!("Attempting connection to MCP server...");
    let client = match try_connect(reqwest::Client::default(), &server_url).await? {
        ConnectOutcome::Connected(client) => {
            tracing::info!("Server accepted the connection without authorization");
            client
        }
        ConnectOutcome::AuthRequired(challenge) => {
            authorize_and_connect(
                challenge,
                oauth_http_client,
                &server_url,
                &client_metadata_url,
                code_receiver,
                &mut output,
            )
            .await?
        }
    };
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
