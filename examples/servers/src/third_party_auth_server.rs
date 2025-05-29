use std::{collections::HashMap, sync::Arc};

use axum::{
    middleware,
    routing::{get, post},
    Json, Router,
};
use rmcp::transport::{
    auth_server::{
        AuthServer, AuthServerConfig, ThirdPartyAuthConfig, TokenInfo, 
        axum_ext, example::{create_oauth_router, create_protected_router},
    },
};
use tokio::net::TcpListener;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use url::Url;


#[derive(serde::Serialize)]
struct MockThirdPartyResponse {
    active: bool,
    scope: String,
    client_id: String,
    username: String,
    exp: u64,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

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
    
    let third_party_config = ThirdPartyAuthConfig {
        client_id: "mcp_client_at_idp".to_string(),
        client_secret: "mcp_client_secret_at_idp".to_string(),
        auth_server_url: "http://localhost:3001".to_string(),
        authorize_endpoint: "http://localhost:3001/oauth2/authorize".to_string(),
        token_endpoint: "http://localhost:3001/oauth2/token".to_string(),
        introspection_endpoint: Some("http://localhost:3001/oauth2/introspect".to_string()),
        revocation_endpoint: Some("http://localhost:3001/oauth2/revoke".to_string()),
        supported_scopes: vec!["openid".to_string(), "profile".to_string(), "mcp".to_string()],
        additional_params: HashMap::new(),
    };
    
    let auth_server = Arc::new(AuthServer::with_third_party_auth(
        config, 
        third_party_config, 
        server_base_url
    ));
    
    let oauth_router = create_oauth_router(auth_server.clone()).await;
    
    let protected_api = create_protected_router(auth_server.clone());
    
    let mcp_app = Router::new()
        .merge(oauth_router)
        .merge(protected_api);
    
    tokio::spawn(start_mock_third_party_server());
    
    let listener = TcpListener::bind("127.0.0.1:3000").await?;
    println!("MCP OAuth server with third-party auth is running at http://localhost:3000");
    println!("This server delegates authentication to the third-party auth server at http://localhost:3001");
    println!("Metadata endpoint: http://localhost:3000/.well-known/oauth-authorization-server");
    println!("Registration endpoint: http://localhost:3000/register");
    println!("Protected API endpoints:");
    println!("  - http://localhost:3000/api/resource (requires any valid token)");
    println!("  - http://localhost:3000/api/mcp (requires 'mcp' scope)");
    println!("  - http://localhost:3000/api/profile (requires 'profile' scope)");
    println!("Test with: curl -H 'Authorization: Bearer test_token_123456' http://localhost:3000/api/resource");
    
    axum::serve(listener, mcp_app).await?;
    Ok(())
}


async fn start_mock_third_party_server() -> anyhow::Result<()> {

    let app = Router::new()
        .route("/oauth2/introspect", post(introspect_handler))
        .route("/oauth2/revoke", post(revoke_handler));
    

    let listener = TcpListener::bind("127.0.0.1:3001").await?;
    println!("Mock third-party OAuth server is running at http://localhost:3001");
    
    axum::serve(listener, app).await?;
    Ok(())
}


async fn introspect_handler(
    axum::Form(params): axum::Form<HashMap<String, String>>
) -> Json<serde::Value> {
    let token = params.get("token").cloned().unwrap_or_default();
    

    if token == "test_token_123456" {

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
            
        Json(serde_json::json!({
            "active": true,
            "scope": "openid profile mcp",
            "client_id": "test_client",
            "username": "test_user",
            "exp": now + 3600,  
        }))
    } else {

        Json(serde_json::json!({
            "active": false
        }))
    }
}


async fn revoke_handler(
    axum::Form(_params): axum::Form<HashMap<String, String>>
) -> &'static str {
    ""
} 