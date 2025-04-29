/// This example shows how to use the RMCP SSE server with OAuth authorization.
/// Use the inspector to view this server https://github.com/modelcontextprotocol/inspector
/// The default index page is available at http://127.0.0.1:8000/
/// # Get a token
/// curl http://127.0.0.1:8000/api/token/demo
/// # Connect to SSE using the token
/// curl -H "Authorization: Bearer demo-token" http://127.0.0.1:8000/sse
use std::{net::SocketAddr, sync::Arc, time::Duration};

use anyhow::Result;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, Request, StatusCode},
    middleware::{self, Next},
    response::{Html, Response},
    routing::get,
};
use rmcp::transport::{SseServer, sse_server::SseServerConfig};
use tokio_util::sync::CancellationToken;
mod common;
use common::counter::Counter;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

const BIND_ADDRESS: &str = "127.0.0.1:8000";
const INDEX_HTML: &str = include_str!("html/sse_auth_index.html");
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

    // Create SSE server configuration
    let sse_config = SseServerConfig {
        bind: addr,
        sse_path: "/sse".to_string(),
        post_path: "/message".to_string(),
        ct: CancellationToken::new(),
        sse_keep_alive: Some(Duration::from_secs(15)),
    };

    // Create SSE server
    let (sse_server, sse_router) = SseServer::new(sse_config);

    // Create API routes
    let api_routes = Router::new()
        .route("/health", get(health_check))
        .route("/token/{token_id}", get(get_token));

    // Create protected SSE routes (require authorization)
    let protected_sse_router = sse_router.layer(middleware::from_fn_with_state(
        token_store.clone(),
        auth_middleware,
    ));

    // Create main router, public endpoints don't require authorization
    let app = Router::new()
        .route("/", get(index))
        .nest("/api", api_routes)
        .merge(protected_sse_router)
        .with_state(());

    // Start server and register service
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let ct = sse_server.config.ct.clone();

    // Start SSE server with Counter service
    sse_server.with_service(Counter::new);

    // Handle signals for graceful shutdown
    let cancel_token = ct.clone();
    tokio::spawn(async move {
        match tokio::signal::ctrl_c().await {
            Ok(()) => {
                println!("Received Ctrl+C, shutting down server...");
                cancel_token.cancel();
            }
            Err(err) => {
                eprintln!("Unable to listen for Ctrl+C signal: {}", err);
            }
        }
    });

    // Start HTTP server
    tracing::info!("Server started on {}", addr);
    let server = axum::serve(listener, app).with_graceful_shutdown(async move {
        // Wait for cancellation signal
        ct.cancelled().await;
        println!("Server is shutting down...");
    });

    if let Err(e) = server.await {
        eprintln!("Server error: {}", e);
    }

    println!("Server has been shut down");
    Ok(())
}
