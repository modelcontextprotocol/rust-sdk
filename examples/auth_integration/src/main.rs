use std::sync::Arc;

use axum::{routing::get, Router};
use rmcp::transport::{
    auth::{OAuthState, AuthorizationManager},
    auth_server::{AuthServer, AuthServerConfig, TokenInfo, axum_ext},
    SseClientTransport, sse_client::SseClientConfig,
};
use tokio::{net::TcpListener, time::sleep, time::Duration};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use url::Url;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();


    tokio::spawn(start_oauth_server());
    
    sleep(Duration::from_secs(1)).await;
    
    run_oauth_client().await?;
    
    Ok(())
}

async fn start_oauth_server() -> anyhow::Result<()> {
    let server_base_url = Url::parse("http://localhost:3000")?;
    

    let config = AuthServerConfig {
        client_id: "mcp_server".to_string(),
        client_secret: "mcp_server_secret".to_string(),
        authorize_endpoint: format!("{}/authorize", server_base_url),
        token_endpoint: format!("{}/token", server_base_url),
        registration_endpoint: format!("{}/register", server_base_url),
        issuer: server_base_url.to_string(),
        supported_scopes: vec![
            "mcp".to_string(), 
            "profile".to_string(), 
            "email".to_string()
        ],
    };
    

    let auth_server = Arc::new(AuthServer::new(config, server_base_url.clone()));
    
    let test_token = "test_token_123456".to_string();
    let token_info = TokenInfo {
        active: true,
        scope: Some("mcp profile".to_string()),
        client_id: "test_client".to_string(),
        username: Some("test_user".to_string()),
        exp: Some(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs() + 3600, // 1小时后过期
        ),
        additional_fields: Default::default(),
    };
    auth_server.register_token(test_token, token_info).await?;
    

    let oauth_router = Router::new()
        .route("/.well-known/oauth-authorization-server", get(axum_ext::oauth_metadata_handler))
        .route("/register", get(axum_ext::client_registration_handler))
        .with_state(auth_server.clone());

    let protected_api = Router::new()
        .route("/api/user", get(user_handler))
        .route_layer(axum::middleware::from_fn_with_state(
            auth_server.clone(),
            axum_ext::auth_middleware,
        ));
    

    let app = Router::new()
        .merge(oauth_router)
        .merge(protected_api);
    
    let listener = TcpListener::bind("127.0.0.1:3000").await?;
    println!("MCP OAuth server is running at http://localhost:3000");
    
    axum::serve(listener, app).await?;
    Ok(())
}


async fn user_handler() -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({
        "id": "user_1",
        "name": "Test User",
        "email": "user@example.com"
    }))
}

async fn run_oauth_client() -> anyhow::Result<()> {
    println!("Starting OAuth client...");
    

    let mut oauth_state = OAuthState::new("http://localhost:3000", None).await?;
    

    let client_id = "test_client";
    let token = rmcp::transport::auth::oauth2::StandardTokenResponse::new(
        rmcp::transport::auth::oauth2::AccessToken::new("test_token_123456".to_string()),
        rmcp::transport::auth::oauth2::basic::BasicTokenType::Bearer,
        rmcp::transport::auth::oauth2::EmptyExtraTokenFields {},
    );
    
    oauth_state.set_credentials(client_id, token).await?;
    
    let auth_manager = oauth_state.into_authorization_manager()
        .ok_or_else(|| anyhow::anyhow!("Failed to get authorization manager"))?;
    
    let client = rmcp::transport::auth::AuthClient::new(reqwest::Client::default(), auth_manager);
    
    let transport = SseClientTransport::start_with_client(
        client,
        SseClientConfig {
            sse_endpoint: "http://localhost:3000/api/user".into(),
            ..Default::default()
        },
    ).await?;
    
    println!("OAuth client successfully connected to server");
    println!("This is a simplified example - in a real-world scenario, the client would go through the full OAuth flow");
    
    Ok(())
} 