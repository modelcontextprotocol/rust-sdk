use std::{collections::HashMap, net::SocketAddr, sync::Arc, time::Duration};

use anyhow::Result;
use askama::Template;
use axum::{
    Json, Router,
    body::Body,
    extract::{Form, Query, State},
    http::{Request, StatusCode},
    middleware::{self, Next},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use rand::{Rng, distr::Alphanumeric};
use rmcp::transport::{
    SseServer,
    auth::{
        AuthorizationMetadata, ClientRegistrationRequest, ClientRegistrationResponse,
        OAuthClientConfig,
    },
    sse_server::SseServerConfig,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tower_http::cors::{Any, CorsLayer};
use tracing::{debug, error, info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use uuid::Uuid;
// Import Counter tool for MCP service
mod common;
use common::counter::Counter;

const BIND_ADDRESS: &str = "127.0.0.1:3000";
const INDEX_HTML: &str = include_str!("html/mcp_oauth_index.html");

// A easy way to manage MCP OAuth Store for managing tokens and sessions
#[derive(Clone, Debug)]
struct McpOAuthStore {
    clients: Arc<RwLock<HashMap<String, OAuthClientConfig>>>,
    auth_sessions: Arc<RwLock<HashMap<String, AuthSession>>>,
    access_tokens: Arc<RwLock<HashMap<String, McpAccessToken>>>,
}

impl McpOAuthStore {
    fn new() -> Self {
        let mut clients = HashMap::new();
        clients.insert(
            "mcp-client".to_string(),
            OAuthClientConfig {
                client_id: "mcp-client".to_string(),
                client_secret: Some("mcp-client-secret".to_string()),
                scopes: vec!["profile".to_string(), "email".to_string()],
                redirect_uri: "http://localhost:8080/callback".to_string(),
            },
        );

        Self {
            clients: Arc::new(RwLock::new(clients)),
            auth_sessions: Arc::new(RwLock::new(HashMap::new())),
            access_tokens: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    async fn validate_client(
        &self,
        client_id: &str,
        redirect_uri: &str,
    ) -> Option<OAuthClientConfig> {
        let clients = self.clients.read().await;
        if let Some(client) = clients.get(client_id) {
            if client.redirect_uri.contains(&redirect_uri.to_string()) {
                return Some(client.clone());
            }
        }
        None
    }

    async fn create_auth_session(
        &self,
        client_id: String,
        scope: Option<String>,
        state: Option<String>,
        session_id: String,
    ) -> String {
        let session = AuthSession {
            client_id,
            scope,
            _state: state,
            _created_at: chrono::Utc::now(),
            auth_token: None,
        };

        self.auth_sessions
            .write()
            .await
            .insert(session_id.clone(), session);
        session_id
    }

    async fn update_auth_session_token(
        &self,
        session_id: &str,
        token: AuthToken,
    ) -> Result<(), String> {
        let mut sessions = self.auth_sessions.write().await;
        if let Some(session) = sessions.get_mut(session_id) {
            session.auth_token = Some(token);
            Ok(())
        } else {
            Err("Session not found".to_string())
        }
    }

    async fn create_mcp_token(&self, session_id: &str) -> Result<McpAccessToken, String> {
        let sessions = self.auth_sessions.read().await;
        if let Some(session) = sessions.get(session_id) {
            if let Some(auth_token) = &session.auth_token {
                let access_token = format!("mcp-token-{}", Uuid::new_v4());
                let token = McpAccessToken {
                    access_token: access_token.clone(),
                    token_type: "Bearer".to_string(),
                    expires_in: 3600,
                    refresh_token: format!("mcp-refresh-{}", Uuid::new_v4()),
                    scope: session.scope.clone(),
                    auth_token: auth_token.clone(),
                    client_id: session.client_id.clone(),
                };

                self.access_tokens
                    .write()
                    .await
                    .insert(access_token.clone(), token.clone());
                Ok(token)
            } else {
                Err("No third-party token available for session".to_string())
            }
        } else {
            Err("Session not found".to_string())
        }
    }

    async fn validate_token(&self, token: &str) -> Option<McpAccessToken> {
        self.access_tokens.read().await.get(token).cloned()
    }
}

// a simple session record for auth session
#[derive(Clone, Debug)]
struct AuthSession {
    client_id: String,
    scope: Option<String>,
    _state: Option<String>,
    _created_at: chrono::DateTime<chrono::Utc>,
    auth_token: Option<AuthToken>,
}

// a simple token record for auth token
// not used oauth2 token for avoid include oauth2 crate in this example
#[derive(Clone, Debug, Serialize, Deserialize)]
struct AuthToken {
    access_token: String,
    token_type: String,
    expires_in: u64,
    refresh_token: String,
    scope: Option<String>,
}

// a simple token record for mcp token ,
// not used oauth2 token for avoid include oauth2 crate in this example
#[derive(Clone, Debug, Serialize)]
struct McpAccessToken {
    access_token: String,
    token_type: String,
    expires_in: u64,
    refresh_token: String,
    scope: Option<String>,
    auth_token: AuthToken,
    client_id: String,
}

#[derive(Debug, Deserialize)]
struct AuthorizeQuery {
    #[allow(dead_code)]
    response_type: String,
    client_id: String,
    redirect_uri: String,
    scope: Option<String>,
    state: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct TokenRequest {
    grant_type: String,
    #[serde(default)]
    code: String,
    #[serde(default)]
    client_id: String,
    #[serde(default)]
    client_secret: String,
    #[serde(default)]
    redirect_uri: String,
    #[serde(default)]
    code_verifier: Option<String>,
    #[serde(default)]
    refresh_token: String,
}

fn generate_random_string(length: usize) -> String {
    rand::rng()
        .sample_iter(&Alphanumeric)
        .take(length)
        .map(char::from)
        .collect()
}

// Root path handler
async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

#[derive(Template)]
#[template(path = "mcp_oauth_authorize.html")]
struct OAuthAuthorizeTemplate {
    client_id: String,
    redirect_uri: String,
    scope: String,
    state: String,
    scopes: String,
}

// Initial OAuth authorize endpoint
async fn oauth_authorize(
    Query(params): Query<AuthorizeQuery>,
    State(state): State<Arc<McpOAuthStore>>,
) -> impl IntoResponse {
    debug!("doing oauth_authorize");
    if let Some(_client) = state
        .validate_client(&params.client_id, &params.redirect_uri)
        .await
    {
        let template = OAuthAuthorizeTemplate {
            client_id: params.client_id,
            redirect_uri: params.redirect_uri,
            scope: params.scope.clone().unwrap_or_default(),
            state: params.state.clone().unwrap_or_default(),
            scopes: params
                .scope
                .clone()
                .unwrap_or_else(|| "Basic scope".to_string()),
        };

        Html(template.render().unwrap()).into_response()
    } else {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "invalid_request",
                "error_description": "invalid client id or redirect uri"
            })),
        )
            .into_response()
    }
}

// handle approval of authorization
#[derive(Debug, Deserialize)]
struct ApprovalForm {
    client_id: String,
    redirect_uri: String,
    scope: String,
    state: String,
    approved: String,
}

async fn oauth_approve(
    State(state): State<Arc<McpOAuthStore>>,
    Form(form): Form<ApprovalForm>,
) -> impl IntoResponse {
    if form.approved != "true" {
        // user rejected the authorization request
        let redirect_url = format!(
            "{}?error=access_denied&error_description={}{}",
            form.redirect_uri,
            "user rejected the authorization request",
            if form.state.is_empty() {
                "".to_string()
            } else {
                format!("&state={}", form.state)
            }
        );
        return Redirect::to(&redirect_url).into_response();
    }

    // user approved the authorization request, generate authorization code
    let session_id = Uuid::new_v4().to_string();
    let auth_code = format!("mcp-code-{}", session_id);

    // create new session record authorization information
    let session_id = state
        .create_auth_session(
            form.client_id.clone(),
            Some(form.scope.clone()),
            Some(form.state.clone()),
            session_id.clone(),
        )
        .await;

    // create token
    let created_token = AuthToken {
        access_token: format!("tp-token-{}", Uuid::new_v4()),
        token_type: "Bearer".to_string(),
        expires_in: 3600,
        refresh_token: format!("tp-refresh-{}", Uuid::new_v4()),
        scope: Some(form.scope),
    };

    // update session token
    if let Err(e) = state
        .update_auth_session_token(&session_id, created_token)
        .await
    {
        error!("Failed to update session token: {}", e);
    }

    // redirect back to client, with authorization code
    let redirect_url = format!(
        "{}?code={}{}",
        form.redirect_uri,
        auth_code,
        if form.state.is_empty() {
            "".to_string()
        } else {
            format!("&state={}", form.state)
        }
    );

    info!("authorization approved, redirecting to: {}", redirect_url);
    Redirect::to(&redirect_url).into_response()
}

// Handle token request from the MCP client
async fn oauth_token(
    State(state): State<Arc<McpOAuthStore>>,
    request: axum::http::Request<Body>,
) -> impl IntoResponse {
    info!("Received token request");

    let bytes = match axum::body::to_bytes(request.into_body(), usize::MAX).await {
        Ok(bytes) => bytes,
        Err(e) => {
            error!("can't read request body: {}", e);
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": "invalid_request",
                    "error_description": "can't read request body"
                })),
            )
                .into_response();
        }
    };

    let body_str = String::from_utf8_lossy(&bytes);
    info!("request body: {}", body_str);

    let token_req = match serde_urlencoded::from_bytes::<TokenRequest>(&bytes) {
        Ok(form) => {
            info!("successfully parsed form data: {:?}", form);
            form
        }
        Err(e) => {
            error!("can't parse form data: {}", e);
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({
                    "error": "invalid_request",
                    "error_description": format!("can't parse form data: {}", e)
                })),
            )
                .into_response();
        }
    };
    if token_req.grant_type == "refresh_token" {
        warn!("this easy server only support authorization_code now");
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "unsupported_grant_type",
                "error_description": "only authorization_code is supported"
            })),
        )
            .into_response();
    }
    if token_req.grant_type != "authorization_code" {
        info!("unsupported grant type: {}", token_req.grant_type);
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "unsupported_grant_type",
                "error_description": "only authorization_code is supported"
            })),
        )
            .into_response();
    }

    // get session_id from code
    if !token_req.code.starts_with("mcp-code-") {
        info!("invalid authorization code: {}", token_req.code);
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "invalid_grant",
                "error_description": "invalid authorization code"
            })),
        )
            .into_response();
    }

    // handle empty client_id
    let client_id = if token_req.client_id.is_empty() {
        "mcp-client".to_string()
    } else {
        token_req.client_id.clone()
    };

    // validate client
    match state
        .validate_client(&client_id, &token_req.redirect_uri)
        .await
    {
        Some(_) => {
            let session_id = token_req.code.replace("mcp-code-", "");
            info!("got session id: {}", session_id);

            // create mcp access token
            match state.create_mcp_token(&session_id).await {
                Ok(token) => {
                    info!("successfully created access token");
                    (
                        StatusCode::OK,
                        Json(serde_json::json!({
                            "access_token": token.access_token,
                            "token_type": token.token_type,
                            "expires_in": token.expires_in,
                            "refresh_token": token.refresh_token,
                            "scope": token.scope,
                        })),
                    )
                        .into_response()
                }
                Err(e) => {
                    error!("failed to create access token: {}", e);
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(serde_json::json!({
                            "error": "server_error",
                            "error_description": format!("failed to create access token: {}", e)
                        })),
                    )
                        .into_response()
                }
            }
        }
        None => {
            info!(
                "invalid client id or redirect uri: {} / {}",
                client_id, token_req.redirect_uri
            );
            (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": "invalid_client",
                    "error_description": "invalid client id or redirect uri"
                })),
            )
                .into_response()
        }
    }
}

// Auth middleware for SSE connections
async fn validate_token_middleware(
    State(token_store): State<Arc<McpOAuthStore>>,
    request: Request<axum::body::Body>,
    next: Next,
) -> Response {
    debug!("validate_token_middleware");
    // Extract the access token from the Authorization header
    let auth_header = request.headers().get("Authorization");
    let token = match auth_header {
        Some(header) => {
            let header_str = header.to_str().unwrap_or("");
            if let Some(stripped) = header_str.strip_prefix("Bearer ") {
                stripped.to_string()
            } else {
                return StatusCode::UNAUTHORIZED.into_response();
            }
        }
        None => {
            return StatusCode::UNAUTHORIZED.into_response();
        }
    };

    // Validate the token
    match token_store.validate_token(&token).await {
        Some(_) => next.run(request).await,
        None => StatusCode::UNAUTHORIZED.into_response(),
    }
}

// handle oauth server metadata request
async fn oauth_authorization_server() -> impl IntoResponse {
    let mut additional_fields = HashMap::new();
    additional_fields.insert(
        "response_types_supported".into(),
        Value::Array(vec![Value::String("code".into())]),
    );
    additional_fields.insert(
        "code_challenge_methods_supported".into(),
        Value::Array(vec![Value::String("S256".into())]),
    );
    let metadata = AuthorizationMetadata {
        authorization_endpoint: format!("http://{}/oauth/authorize", BIND_ADDRESS),
        token_endpoint: format!("http://{}/oauth/token", BIND_ADDRESS),
        scopes_supported: Some(vec!["profile".to_string(), "email".to_string()]),
        registration_endpoint: format!("http://{}/oauth/register", BIND_ADDRESS),
        issuer: Some(BIND_ADDRESS.to_string()),
        jwks_uri: Some(format!("http://{}/oauth/jwks", BIND_ADDRESS)),
        additional_fields,
    };
    debug!("metadata: {:?}", metadata);
    (StatusCode::OK, Json(metadata))
}

// handle client registration request
async fn oauth_register(
    State(state): State<Arc<McpOAuthStore>>,
    Json(req): Json<ClientRegistrationRequest>,
) -> impl IntoResponse {
    debug!("register request: {:?}", req);
    if req.redirect_uris.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "invalid_request",
                "error_description": "at least one redirect uri is required"
            })),
        )
            .into_response();
    }

    // generate client id and secret
    let client_id = format!("client-{}", Uuid::new_v4());
    let client_secret = generate_random_string(32);

    let client = OAuthClientConfig {
        client_id: client_id.clone(),
        client_secret: Some(client_secret.clone()),
        redirect_uri: req.redirect_uris[0].clone(),
        scopes: vec![],
    };

    state
        .clients
        .write()
        .await
        .insert(client_id.clone(), client);

    // return client information
    let response = ClientRegistrationResponse {
        client_id,
        client_secret: Some(client_secret),
        client_name: req.client_name,
        redirect_uris: req.redirect_uris,
        additional_fields: HashMap::new(),
    };

    (StatusCode::CREATED, Json(response)).into_response()
}

// Log all HTTP requests
async fn log_request(request: Request<Body>, next: Next) -> Response {
    let method = request.method().clone();
    let uri = request.uri().clone();
    let version = request.version();

    // Log headers
    let headers = request.headers().clone();
    let mut header_log = String::new();
    for (key, value) in headers.iter() {
        let value_str = value.to_str().unwrap_or("<binary>");
        header_log.push_str(&format!("\n  {}: {}", key, value_str));
    }

    // Try to get request body for form submissions
    let content_type = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let request_info = if content_type.contains("application/x-www-form-urlencoded")
        || content_type.contains("application/json")
    {
        format!(
            "{} {} {:?}{}\nContent-Type: {}",
            method, uri, version, header_log, content_type
        )
    } else {
        format!("{} {} {:?}{}", method, uri, version, header_log)
    };

    info!("REQUEST: {}", request_info);

    // Call the actual handler
    let response = next.run(request).await;

    // Log response status
    let status = response.status();
    info!("RESPONSE: {} for {} {}", status, method, uri);

    response
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

    // Create the OAuth store
    let oauth_store = Arc::new(McpOAuthStore::new());

    // Set up port
    let addr = BIND_ADDRESS.parse::<SocketAddr>()?;

    // Create SSE server configuration for MCP
    let sse_config = SseServerConfig {
        bind: addr,
        sse_path: "/mcp/sse".to_string(),
        post_path: "/mcp/message".to_string(),
        ct: CancellationToken::new(),
        sse_keep_alive: Some(Duration::from_secs(15)),
    };

    // Create SSE server
    let (sse_server, sse_router) = SseServer::new(sse_config);

    // Create protected SSE routes (require authorization)
    let protected_sse_router = sse_router.layer(middleware::from_fn_with_state(
        oauth_store.clone(),
        validate_token_middleware,
    ));

    // Create CORS layer for the oauth authorization server endpoint
    let cors_layer = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    // Create a sub-router for the oauth authorization server endpoint with CORS
    let oauth_server_router = Router::new()
        .route(
            "/.well-known/oauth-authorization-server",
            get(oauth_authorization_server).options(oauth_authorization_server),
        )
        .route("/oauth/token", post(oauth_token).options(oauth_token))
        .route(
            "/oauth/register",
            post(oauth_register).options(oauth_register),
        )
        .layer(cors_layer)
        .with_state(oauth_store.clone());

    // Create HTTP router with request logging middleware
    let app = Router::new()
        .route("/", get(index))
        .route("/mcp", get(index))
        .route("/oauth/authorize", get(oauth_authorize))
        .route("/oauth/approve", post(oauth_approve))
        .merge(oauth_server_router) // Merge the CORS-enabled oauth server router
        // .merge(protected_sse_router)
        .with_state(oauth_store.clone())
        .layer(middleware::from_fn(log_request));

    let app = app.merge(protected_sse_router);
    // Register token validation middleware for SSE
    let cancel_token = sse_server.config.ct.clone();
    // Handle Ctrl+C
    let cancel_token2 = sse_server.config.ct.clone();
    // Start SSE server with Counter service
    sse_server.with_service(Counter::new);

    // Start HTTP server
    info!("MCP OAuth Server started on {}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let server = axum::serve(listener, app).with_graceful_shutdown(async move {
        cancel_token.cancelled().await;
        info!("Server is shutting down");
    });

    tokio::spawn(async move {
        match tokio::signal::ctrl_c().await {
            Ok(()) => {
                info!("Received Ctrl+C, shutting down");
                cancel_token2.cancel();
            }
            Err(e) => error!("Failed to listen for Ctrl+C: {}", e),
        }
    });

    if let Err(e) = server.await {
        error!("Server error: {}", e);
    }

    Ok(())
}
