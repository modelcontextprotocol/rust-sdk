use std::{collections::HashMap, net::SocketAddr, sync::Arc, time::Duration};

use anyhow::Result;
use axum::{
    Json, Router,
    extract::{Form, Path, Query, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use rand::{Rng, distributions::Alphanumeric};
use reqwest::Client as HttpClient;
use rmcp::transport::{
    SseServer,
    auth::{
        AuthorizationMetadata, ClientRegistrationRequest, ClientRegistrationResponse,
        OAuthClientConfig,
    },
    sse_server::SseServerConfig,
};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use uuid::Uuid;
// Import Counter tool for MCP service
mod common;
use common::counter::Counter;

const BIND_ADDRESS: &str = "127.0.0.1:3000";

// MCP OAuth Store for managing tokens and sessions
#[derive(Clone, Debug)]
struct McpOAuthStore {
    clients: Arc<RwLock<HashMap<String, OAuthClientConfig>>>,
    auth_sessions: Arc<RwLock<HashMap<String, AuthSession>>>,
    access_tokens: Arc<RwLock<HashMap<String, McpAccessToken>>>,
    http_client: HttpClient,
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
            http_client: HttpClient::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .expect("Failed to create HTTP client"),
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
        redirect_uri: String,
        scope: Option<String>,
        state: Option<String>,
    ) -> String {
        let session_id = generate_random_string(16);
        let session = AuthSession {
            id: session_id.clone(),
            client_id,
            redirect_uri,
            scope,
            state,
            created_at: chrono::Utc::now(),
            third_party_token: None,
        };

        self.auth_sessions
            .write()
            .await
            .insert(session_id.clone(), session);
        session_id
    }

    async fn get_auth_session(&self, session_id: &str) -> Option<AuthSession> {
        self.auth_sessions.read().await.get(session_id).cloned()
    }

    async fn update_auth_session_token(
        &self,
        session_id: &str,
        token: ThirdPartyToken,
    ) -> Result<(), String> {
        let mut sessions = self.auth_sessions.write().await;
        if let Some(session) = sessions.get_mut(session_id) {
            session.third_party_token = Some(token);
            Ok(())
        } else {
            Err("Session not found".to_string())
        }
    }

    async fn create_mcp_token(&self, session_id: &str) -> Result<McpAccessToken, String> {
        let sessions = self.auth_sessions.read().await;
        if let Some(session) = sessions.get(session_id) {
            if let Some(third_party_token) = &session.third_party_token {
                let access_token = format!("mcp-token-{}", Uuid::new_v4());
                let token = McpAccessToken {
                    access_token: access_token.clone(),
                    token_type: "Bearer".to_string(),
                    expires_in: 3600,
                    refresh_token: format!("mcp-refresh-{}", Uuid::new_v4()),
                    scope: session.scope.clone(),
                    third_party_token: third_party_token.clone(),
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

#[derive(Clone, Debug)]
struct AuthSession {
    id: String,
    client_id: String,
    redirect_uri: String,
    scope: Option<String>,
    state: Option<String>,
    created_at: chrono::DateTime<chrono::Utc>,
    third_party_token: Option<ThirdPartyToken>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ThirdPartyToken {
    access_token: String,
    token_type: String,
    expires_in: u64,
    refresh_token: String,
    scope: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
struct McpAccessToken {
    access_token: String,
    token_type: String,
    expires_in: u64,
    refresh_token: String,
    scope: Option<String>,
    third_party_token: ThirdPartyToken,
    client_id: String,
}

#[derive(Debug, Deserialize)]
struct AuthorizeQuery {
    response_type: String,
    client_id: String,
    redirect_uri: String,
    scope: Option<String>,
    state: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AuthCallbackQuery {
    code: String,
    state: Option<String>,
    session_id: String,
}

#[derive(Debug, Deserialize)]
struct TokenRequest {
    grant_type: String,
    code: String,
    client_id: String,
    client_secret: String,
    redirect_uri: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct ThirdPartyTokenRequest {
    grant_type: String,
    code: String,
    client_id: String,
    client_secret: String,
    redirect_uri: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct UserInfo {
    sub: String,
    name: String,
    email: String,
    username: String,
}

fn generate_random_string(length: usize) -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(length)
        .map(char::from)
        .collect()
}

// Root path handler
async fn index() -> Html<&'static str> {
    Html(
        r#"
    <!DOCTYPE html>
    <html>
    <head>
        <title>MCP OAuth Server</title>
        <style>
            body { font-family: Arial, sans-serif; margin: 40px auto; max-width: 800px; line-height: 1.6; }
            h1, h2 { color: #333; }
            code { background: #f4f4f4; padding: 2px 5px; border-radius: 3px; }
            .endpoint { background: #f9f9f9; padding: 15px; border-radius: 5px; margin-bottom: 15px; }
            .flow { background: #e8f5e9; padding: 15px; border-radius: 5px; margin-bottom: 15px; }
        </style>
    </head>
    <body>
        <h1>MCP OAuth Server</h1>
        <p>This is an MCP server with OAuth 2.0 integration to a third-party authorization server.</p>
        
        <h2>Available Endpoints:</h2>
        
        <div class="endpoint">
            <h3>Authorization Endpoint</h3>
            <p><code>GET /oauth/authorize</code></p>
            <p>Parameters:</p>
            <ul>
                <li><code>response_type</code> - Must be "code"</li>
                <li><code>client_id</code> - Client identifier (e.g., "mcp-client")</li>
                <li><code>redirect_uri</code> - URI to redirect after authorization</li>
                <li><code>scope</code> - Optional requested scope</li>
                <li><code>state</code> - Optional state value for CSRF prevention</li>
            </ul>
        </div>
        
        <div class="endpoint">
            <h3>Token Endpoint</h3>
            <p><code>POST /oauth/token</code></p>
            <p>Parameters:</p>
            <ul>
                <li><code>grant_type</code> - Must be "authorization_code"</li>
                <li><code>code</code> - The authorization code</li>
                <li><code>client_id</code> - Client identifier</li>
                <li><code>client_secret</code> - Client secret</li>
                <li><code>redirect_uri</code> - Redirect URI used in authorization request</li>
            </ul>
        </div>
        
        <div class="endpoint">
            <h3>MCP SSE Endpoints</h3>
            <p><code>/mcp/sse</code> - SSE connection endpoint (requires OAuth token)</p>
            <p><code>/mcp/message</code> - Message endpoint (requires OAuth token)</p>
        </div>
        
        <div class="flow">
            <h2>OAuth Flow:</h2>
            <ol>
                <li>MCP Client initiates OAuth flow with this MCP Server</li>
                <li>MCP Server redirects to Third-Party OAuth Server</li>
                <li>User authenticates with Third-Party Server</li>
                <li>Third-Party Server redirects back to MCP Server with auth code</li>
                <li>MCP Server exchanges the code for a third-party access token</li>
                <li>MCP Server generates its own token bound to the third-party session</li>
                <li>MCP Server completes the OAuth flow with the MCP Client</li>
            </ol>
        </div>
    </body>
    </html>
    "#,
    )
}

// Initial OAuth authorize endpoint
async fn oauth_authorize(
    Query(params): Query<AuthorizeQuery>,
    State(state): State<McpOAuthStore>,
) -> impl IntoResponse {
    if let Some(client) = state
        .validate_client(&params.client_id, &params.redirect_uri)
        .await
    {
        // create authorize page for user to approve
        let html = format!(
            r#"
            <!DOCTYPE html>
            <html>
            <head>
                <title>MCP OAuth</title>
                <style>
                    body {{ font-family: Arial, sans-serif; margin: 40px auto; max-width: 600px; line-height: 1.6; }}
                    h1 {{ color: #333; }}
                    .container {{ background: #f9f9f9; padding: 20px; border-radius: 5px; border: 1px solid #ddd; }}
                    .btn-group {{ margin-top: 20px; }}
                    .btn {{ padding: 10px 15px; border-radius: 3px; cursor: pointer; margin-right: 10px; }}
                    .btn-primary {{ background: #4285f4; color: white; border: none; }}
                    .btn-secondary {{ background: #f1f1f1; color: #333; border: 1px solid #ddd; }}
                    .client-info {{ margin-bottom: 20px; padding: 10px; background: #f0f0f0; border-radius: 3px; }}
                </style>
            </head>
            <body>
                <h1>MCP OAuth Server</h1>
                <div class="container">
                    <div class="client-info">
                        <p><strong>{client_id}</strong> requests access to your account.</p>
                        <p>requested scopes: {scopes}</p>
                    </div>
                    
                    <form action="/oauth/approve" method="post">
                        <input type="hidden" name="client_id" value="{client_id}">
                        <input type="hidden" name="redirect_uri" value="{redirect_uri}">
                        <input type="hidden" name="scope" value="{scope}">
                        <input type="hidden" name="state" value="{state}">
                        
                        <div class="btn-group">
                            <button type="submit" name="approved" value="true" class="btn btn-primary">Approve</button>
                            <button type="submit" name="approved" value="false" class="btn btn-secondary">Reject</button>
                        </div>
                    </form>
                </div>
            </body>
            </html>
            "#,
            client_id = params.client_id,
            redirect_uri = params.redirect_uri,
            scope = params.scope.clone().unwrap_or_default(),
            state = params.state.clone().unwrap_or_default(),
            scopes = params
                .scope
                .clone()
                .unwrap_or_else(|| "basic access".to_string()),
        );

        Html(html).into_response()
    } else {
        // invalid client_id or redirect_uri
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
    State(state): State<McpOAuthStore>,
    Form(form): Form<ApprovalForm>,
) -> impl IntoResponse {
    if form.approved != "true" {
        // user rejected the authorization request
        let redirect_url = format!(
            "{}?error=access_denied&error_description={}{}",
            form.redirect_uri,
            "user rejected the authorization request",
            form.state
                .is_empty()
                .then_some("")
                .unwrap_or(&format!("&state={}", form.state))
        );
        return Redirect::to(&redirect_url).into_response();
    }

    // user approved the authorization request, generate authorization code
    let auth_code = format!("mcp-code-{}", Uuid::new_v4().to_string());

    // create new session record authorization information
    let session_id = state
        .create_auth_session(
            form.client_id,
            form.redirect_uri.clone(),
            Some(form.scope),
            Some(form.state.clone()),
        )
        .await;

    // redirect back to client, with authorization code
    let redirect_url = format!(
        "{}?code={}{}",
        form.redirect_uri,
        auth_code,
        form.state
            .is_empty()
            .then_some("")
            .unwrap_or(&format!("&state={}", form.state))
    );

    info!("authorization approved, redirecting to: {}", redirect_url);
    Redirect::to(&redirect_url).into_response()
}

// Handle token request from the MCP client
async fn oauth_token(
    State(state): State<McpOAuthStore>,
    Form(token_req): Form<TokenRequest>,
) -> impl IntoResponse {
    if token_req.grant_type != "authorization_code" {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "unsupported_grant_type"
            })),
        )
            .into_response();
    }

    // Validate the client
    if let Some(_client) = state
        .validate_client(&token_req.client_id, &token_req.redirect_uri)
        .await
    {
        // The code we generated earlier is "mcp-code-{session_id}"
        if !token_req.code.starts_with("mcp-code-") {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": "invalid_grant",
                    "error_description": "Invalid authorization code"
                })),
            )
                .into_response();
        }

        let session_id = token_req.code.replace("mcp-code-", "");

        // Create an MCP access token bound to the third-party token
        match state.create_mcp_token(&session_id).await {
            Ok(token) => {
                // Return the token
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
                error!("Failed to create MCP token: {}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({
                        "error": "server_error",
                        "error_description": "Failed to create access token"
                    })),
                )
                    .into_response()
            }
        }
    } else {
        // Invalid client credentials
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "invalid_client"
            })),
        )
            .into_response()
    }
}

// Auth middleware for SSE connections
async fn validate_token_middleware(
    headers: HeaderMap,
    State(state): State<McpOAuthStore>,
) -> Result<Option<String>, StatusCode> {
    // Extract the access token from the Authorization header
    let auth_header = headers.get("Authorization");
    let token = match auth_header {
        Some(header) => {
            let header_str = header.to_str().unwrap_or("");
            if header_str.starts_with("Bearer ") {
                header_str[7..].to_string()
            } else {
                return Err(StatusCode::UNAUTHORIZED);
            }
        }
        None => {
            return Err(StatusCode::UNAUTHORIZED);
        }
    };

    // Validate the token
    match state.validate_token(&token).await {
        Some(_) => Ok(Some(token)),
        None => Err(StatusCode::UNAUTHORIZED),
    }
}

// handle oauth server metadata request
async fn oauth_authorization_server() -> impl IntoResponse {
    let metadata = AuthorizationMetadata {
        authorization_endpoint: format!("http://{}/oauth/authorize", BIND_ADDRESS),
        token_endpoint: format!("http://{}/oauth/token", BIND_ADDRESS),
        scopes_supported: Some(vec!["profile".to_string(), "email".to_string()]),
        registration_endpoint: format!("http://{}/oauth/register", BIND_ADDRESS),
        issuer: Some(format!("{}", BIND_ADDRESS)),
        jwks_uri: Some(format!("http://{}/oauth/jwks", BIND_ADDRESS)),
    };
    debug!("metadata: {:?}", metadata);
    (StatusCode::OK, Json(metadata))
}

// handle client registration request
async fn oauth_register(
    State(state): State<McpOAuthStore>,
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
        client_secret,
        client_name: req.client_name,
        redirect_uris: req.redirect_uris,
    };

    (StatusCode::CREATED, Json(response)).into_response()
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
    let oauth_store = McpOAuthStore::new();

    // Set up port
    let addr = BIND_ADDRESS.parse::<SocketAddr>()?;

    // Create SSE server configuration for MCP
    let sse_config = SseServerConfig {
        bind: addr.clone(),
        sse_path: "/mcp/sse".to_string(),
        post_path: "/mcp/message".to_string(),
        ct: CancellationToken::new(),
        sse_keep_alive: Some(Duration::from_secs(15)),
    };

    // Create SSE server
    let (sse_server, sse_router) = SseServer::new(sse_config);

    // Create HTTP router
    let app = Router::new()
        .route("/", get(index))
        .route("/mcp", get(index))
        .route(
            "/.well-known/oauth-authorization-server",
            get(oauth_authorization_server),
        )
        .route("/oauth/authorize", get(oauth_authorize))
        .route("/oauth/approve", post(oauth_approve))
        .route("/oauth/token", post(oauth_token))
        .route("/oauth/register", post(oauth_register))
        .with_state(oauth_store.clone());

    let app = app.merge(sse_router.with_state(()));
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
