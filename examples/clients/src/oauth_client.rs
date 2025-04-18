use std::{sync::Arc, time::Duration};

use anyhow::Result;
use rmcp::{
    RoleClient, ServiceExt,
    model::ClientInfo,
    transport::{
        auth::{AuthError, AuthorizationManager, AuthorizationSession, AuthorizedHttpClient},
        create_authorized_transport,
        sse::SseTransportRetryConfig,
    },
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, BufWriter},
    sync::Mutex,
};
use url::Url;

#[tokio::main]
async fn main() -> Result<()> {
    // init logger
    tracing_subscriber::fmt::init();

    // server url
    let server_url =
        std::env::var("MCP_SERVER_URL").unwrap_or_else(|_| "http://localhost:3000/mcp".to_string());

    // retry config
    let retry_config = SseTransportRetryConfig {
        max_times: Some(3),
        min_duration: Duration::from_secs(1),
    };

    // init auth manager
    let auth_manager = AuthorizationManager::new(&server_url).await?;
    let auth_manager_arc = Arc::new(Mutex::new(auth_manager));

    // create authorization session
    let session = AuthorizationSession::new(
        auth_manager_arc.clone(),
        &["mcp"],                         // request scopes
        "http://localhost:8080/callback", // redirect uri
    )
    .await?;

    // output authorization url
    let mut output = BufWriter::new(tokio::io::stdout());
    output
        .write_all(b"please open the following URL in your browser:\n")
        .await?;
    output
        .write_all(session.get_authorization_url().as_bytes())
        .await?;
    output
        .write_all(b"\nplease input the authorization code:\n")
        .await?;
    output.flush().await?;

    // read authorization code
    let mut auth_code = String::new();
    let mut reader = BufReader::new(tokio::io::stdin());
    reader.read_line(&mut auth_code).await?;
    let auth_code = auth_code.trim();

    // exchange access token
    let credentials = session.handle_callback(auth_code).await?;
    tracing::info!("Successfully obtained access token");

    // create authorized sse transport, use retry config
    let transport =
        create_authorized_transport(&server_url, auth_manager_arc, Some(retry_config)).await?;

    // create client
    let client_service = ClientInfo::default();
    let client = client_service.serve(transport).await?;

    // test api request
    let tools = client.peer().list_all_tools().await?;
    tracing::info!("Available tools: {tools:#?}");

    // get prompt list
    let prompts = client.peer().list_all_prompts().await?;
    tracing::info!("Available prompts: {prompts:#?}");

    // get resource list
    let resources = client.peer().list_all_resources().await?;
    tracing::info!("Available resources: {resources:#?}");

    Ok(())
}
