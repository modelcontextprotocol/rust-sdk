/// This example shows how to use the RMCP streamable HTTP server with simple token authorization.
/// Use the inspector to view this server https://github.com/modelcontextprotocol/inspector
/// The default index page is available at http://127.0.0.1:8000/
/// # Get a token
/// curl http://127.0.0.1:8000/api/token/demo
/// # Connect using the token
/// curl -X POST -H "Authorization: Bearer demo-token" -H "Content-Type: application/json" \
///   -d '{"jsonrpc":"2.0","method":"initialize","params":{},"id":1}' \
///   http://127.0.0.1:8000/mcp
use std::{net::SocketAddr, sync::Arc};

use anyhow::Result;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, Request, StatusCode},
    middleware::{self, Next},
    response::{Html, Response},
    routing::get,
};
use rmcp::transport::{
    StreamableHttpServerConfig,
    streamable_http_server::{session::local::LocalSessionManager, tower::StreamableHttpService},
};
mod common;
use common::counter::Counter;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

const BIND_ADDRESS: &str = "127.0.0.1:8000";
const INDEX_HTML: &str = r#"<!DOCTYPE html>
<html>
<head>
    <title>MCP Streamable HTTP Auth Server</title>
</head>
<body>
    <h1>MCP Streamable HTTP Server with Auth</h1>
    <p>Get a token: <code>curl http://127.0.0.1:8000/api/token/demo</code></p>
    <p>Connect: <code>curl -X POST -H "Authorization: Bearer demo-token" -H "Content-Type: application/json" -d '{"jsonrpc":"2.0","method":"initialize","params":{},"id":1}' http://127.0.0.1:8000/mcp</code></p>
</body>
</html>"#;

// A simple token store
struct TokenStore {
    valid_tokens: Vec<String>,
}

impl TokenStore {
    fn new() -> Self {
        // For demonstration purposes, use more secure token management in production
        Self {
            valid_tokens: vec!["demo-token".to_string(), "test-token".to_string()],
        }
    }

    fn is_valid(&self, token: &str) -> bool {
        self.valid_tokens.contains(&token.to_string())
    }
}

// Extract authorization token
fn extract_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get("Authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|auth_header| {
            auth_header
                .strip_prefix("Bearer ")
                .map(|stripped| stripped.to_string())
        })
}

// Authorization middleware
async fn auth_middleware(
    State(token_store): State<Arc<TokenStore>>,
    headers: HeaderMap,
    request: Request<axum::body::Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    match extract_token(&headers) {
        Some(token) if token_store.is_valid(&token) => {
            // Token is valid, proceed with the request
            Ok(next.run(request).await)
        }
        _ => {
            // Token is invalid, return 401 error
            Err(StatusCode::UNAUTHORIZED)
        }
    }
}

// Root path handler
async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

// Health check endpoint
async fn health_check() -> &'static str {
    "OK"
}

// Token generation endpoint (simplified example)
async fn get_token(Path(token_id): Path<String>) -> Result<Json<serde_json::Value>, StatusCode> {
    // In a real application, you should authenticate the user and generate a real token
    if token_id == "demo" || token_id == "test" {
        let token = format!("{}-token", token_id);
        Ok(Json(serde_json::json!({
            "access_token": token,
            "token_type": "Bearer",
            "expires_in": 3600
        })))
    } else {
        Err(StatusCode::UNAUTHORIZED)
    }
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

    // Create token store
    let token_store = Arc::new(TokenStore::new());

    // Set up port
    let addr = BIND_ADDRESS.parse::<SocketAddr>()?;

    // Create streamable HTTP service
    let mcp_service: StreamableHttpService<Counter, LocalSessionManager> =
        StreamableHttpService::new(
            || Ok(Counter::new()),
            LocalSessionManager::default().into(),
            StreamableHttpServerConfig::default(),
        );

    // Create API routes
    let api_routes = Router::new()
        .route("/health", get(health_check))
        .route("/token/{token_id}", get(get_token));

    // Create protected MCP routes (require authorization)
    let protected_mcp_router =
        Router::new()
            .nest_service("/mcp", mcp_service)
            .layer(middleware::from_fn_with_state(
                token_store.clone(),
                auth_middleware,
            ));

    // Create main router, public endpoints don't require authorization
    let app = Router::new()
        .route("/", get(index))
        .nest("/api", api_routes)
        .merge(protected_mcp_router);

    // Start HTTP server
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("Server started on {}", addr);

    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            tokio::signal::ctrl_c().await.ok();
            println!("Shutting down...");
        })
        .await?;

    Ok(())
}
