use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use oauth2::{
    AuthUrl, ClientId, ClientSecret, RedirectUrl, TokenUrl,
    basic::{BasicClient, BasicTokenType},
};
use reqwest::{StatusCode, Url, header::AUTHORIZATION};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::RwLock;
use tracing::{debug, error, info};

use crate::transport::auth::AuthError;

/// Server OAuth error types
#[derive(Debug, Error)]
pub enum ServerAuthError {
    #[error("Invalid token: {0}")]
    InvalidToken(String),
    
    #[error("Token expired")]
    TokenExpired,
    
    #[error("Insufficient scope: {0}")]
    InsufficientScope(String),
    
    #[error("Invalid client: {0}")]
    InvalidClient(String),
    
    #[error("Internal error: {0}")]
    InternalError(String),
    
    #[error("HTTP error: {0}")]
    HttpError(#[from] reqwest::Error),
    
    #[error("URL parse error: {0}")]
    UrlError(#[from] url::ParseError),

    #[error("Third party authorization error: {0}")]
    ThirdPartyAuthError(String),
}

/// Represents an OAuth server configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthServerConfig {
    pub client_id: String,
    pub client_secret: String,
    pub authorize_endpoint: String,
    pub token_endpoint: String,
    pub registration_endpoint: String,
    pub issuer: String,
    pub supported_scopes: Vec<String>,
}

/// Third party authorization server configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThirdPartyAuthConfig {
    /// Client ID to use with the third-party auth server
    pub client_id: String,
    /// Client secret to use with the third-party auth server
    pub client_secret: String,
    /// URL of the third-party authorization server
    pub auth_server_url: String,
    /// Authorization endpoint at the third-party server
    pub authorize_endpoint: String,
    /// Token endpoint at the third-party server
    pub token_endpoint: String,
    /// Token introspection endpoint at the third-party server
    pub introspection_endpoint: Option<String>,
    /// Token revocation endpoint at the third-party server
    pub revocation_endpoint: Option<String>,
    /// Scopes supported by the third-party server
    pub supported_scopes: Vec<String>,
    /// Additional parameters to include in auth requests
    pub additional_params: HashMap<String, String>,
}

/// Represents OAuth server metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthServerMetadata {
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub registration_endpoint: String,
    pub issuer: String,
    pub scopes_supported: Vec<String>,
    #[serde(flatten)]
    pub additional_fields: HashMap<String, serde_json::Value>,
}

/// Dynamic client registration request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientRegistrationRequest {
    pub client_name: String,
    pub redirect_uris: Vec<String>,
    pub grant_types: Vec<String>,
    pub token_endpoint_auth_method: String,
    pub response_types: Vec<String>,
}

/// Dynamic client registration response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientRegistrationResponse {
    pub client_id: String,
    pub client_secret: Option<String>,
    pub client_name: String,
    pub redirect_uris: Vec<String>,
    #[serde(flatten)]
    pub additional_fields: HashMap<String, serde_json::Value>,
}

/// Token info for validation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenInfo {
    pub active: bool,
    pub scope: Option<String>,
    pub client_id: String,
    pub username: Option<String>,
    pub exp: Option<u64>,
    #[serde(flatten)]
    pub additional_fields: HashMap<String, serde_json::Value>,
}

/// Third-party token introspection request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenIntrospectionRequest {
    pub token: String,
    pub token_type_hint: Option<String>,
}

/// Third-party token introspection response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenIntrospectionResponse {
    pub active: bool,
    pub scope: Option<String>,
    pub client_id: Option<String>,
    pub username: Option<String>,
    pub exp: Option<u64>,
    #[serde(flatten)]
    pub additional_fields: HashMap<String, serde_json::Value>,
}

/// OAuth server implementation
pub struct AuthServer {
    config: AuthServerConfig,
    third_party_config: Option<ThirdPartyAuthConfig>,
    registered_clients: RwLock<HashMap<String, ClientRegistrationResponse>>,
    active_tokens: RwLock<HashMap<String, TokenInfo>>,
    server_base_url: Url,
    http_client: reqwest::Client,
}

impl AuthServer {
    /// Create a new OAuth server
    pub fn new(config: AuthServerConfig, server_base_url: Url) -> Self {
        Self {
            config,
            third_party_config: None,
            registered_clients: RwLock::new(HashMap::new()),
            active_tokens: RwLock::new(HashMap::new()),
            server_base_url,
            http_client: reqwest::Client::new(),
        }
    }

    /// Create a new OAuth server with third-party authorization
    pub fn with_third_party_auth(
        config: AuthServerConfig, 
        third_party_config: ThirdPartyAuthConfig,
        server_base_url: Url
    ) -> Self {
        Self {
            config,
            third_party_config: Some(third_party_config),
            registered_clients: RwLock::new(HashMap::new()),
            active_tokens: RwLock::new(HashMap::new()),
            server_base_url,
            http_client: reqwest::Client::new(),
        }
    }

    /// Check if server is using third-party auth
    pub fn is_using_third_party_auth(&self) -> bool {
        self.third_party_config.is_some()
    }

    /// Generate metadata document
    pub fn generate_metadata(&self) -> AuthServerMetadata {
        AuthServerMetadata {
            authorization_endpoint: self.config.authorize_endpoint.clone(),
            token_endpoint: self.config.token_endpoint.clone(),
            registration_endpoint: self.config.registration_endpoint.clone(),
            issuer: self.config.issuer.clone(),
            scopes_supported: self.config.supported_scopes.clone(),
            additional_fields: HashMap::new(),
        }
    }

    /// Handle client registration
    pub async fn handle_registration(
        &self,
        request: ClientRegistrationRequest,
    ) -> Result<ClientRegistrationResponse, ServerAuthError> {
        debug!("Handling client registration request: {:?}", request);
        
        // Validate redirect URIs
        for uri in &request.redirect_uris {
            let url = Url::parse(uri).map_err(|e| {
                ServerAuthError::InvalidClient(format!("Invalid redirect URI {}: {}", uri, e))
            })?;
            
            // Validate according to MCP spec (must be localhost or HTTPS)
            if url.scheme() != "https" && !(url.host_str() == Some("localhost") || url.host_str() == Some("127.0.0.1")) {
                return Err(ServerAuthError::InvalidClient(
                    format!("Redirect URI must use HTTPS or localhost: {}", uri)
                ));
            }
        }
        
        // Generate client ID and optional secret
        let client_id = format!("client_{}", uuid::Uuid::new_v4().to_string().replace("-", ""));
        let client_secret = Some(format!("secret_{}", uuid::Uuid::new_v4().to_string().replace("-", "")));
        
        let response = ClientRegistrationResponse {
            client_id: client_id.clone(),
            client_secret,
            client_name: request.client_name,
            redirect_uris: request.redirect_uris,
            additional_fields: HashMap::new(),
        };
        
        // Store the registered client
        self.registered_clients.write().await.insert(client_id, response.clone());
        
        debug!("Client registered successfully: {}", response.client_id);
        Ok(response)
    }
    
    /// Validate token
    pub async fn validate_token(&self, token: &str) -> Result<TokenInfo, ServerAuthError> {
        // If third-party auth is configured, use that for validation
        if let Some(third_party_config) = &self.third_party_config {
            return self.validate_token_with_third_party(token, third_party_config).await;
        }
        
        // Otherwise, use local validation
        let active_tokens = self.active_tokens.read().await;
        
        if let Some(token_info) = active_tokens.get(token) {
            // Check if token is expired
            if let Some(exp) = token_info.exp {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                
                if now > exp {
                    return Err(ServerAuthError::TokenExpired);
                }
            }
            
            if token_info.active {
                Ok(token_info.clone())
            } else {
                Err(ServerAuthError::InvalidToken("Token is inactive".to_string()))
            }
        } else {
            Err(ServerAuthError::InvalidToken("Unknown token".to_string()))
        }
    }
    
    /// Validate token with third-party auth server
    async fn validate_token_with_third_party(
        &self,
        token: &str,
        config: &ThirdPartyAuthConfig,
    ) -> Result<TokenInfo, ServerAuthError> {
        // Check if introspection endpoint is available
        let introspection_endpoint = config.introspection_endpoint.as_ref().ok_or_else(|| {
            ServerAuthError::ThirdPartyAuthError("No introspection endpoint configured".to_string())
        })?;
        
        // Prepare introspection request
        let introspection_request = TokenIntrospectionRequest {
            token: token.to_string(),
            token_type_hint: Some("access_token".to_string()),
        };
        
        // Make request to third-party server
        let response = self.http_client
            .post(introspection_endpoint)
            .basic_auth(&config.client_id, Some(&config.client_secret))
            .form(&introspection_request)
            .send()
            .await
            .map_err(|e| ServerAuthError::HttpError(e))?;
        
        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_else(|_| "Unknown error".to_string());
            return Err(ServerAuthError::ThirdPartyAuthError(
                format!("Introspection failed: HTTP {} - {}", status, error_text)
            ));
        }
        
        // Parse introspection response
        let introspection: TokenIntrospectionResponse = response.json().await
            .map_err(|e| ServerAuthError::ThirdPartyAuthError(
                format!("Failed to parse introspection response: {}", e)
            ))?;
        
        if !introspection.active {
            return Err(ServerAuthError::InvalidToken("Token is inactive".to_string()));
        }
        
        // Check expiration
        if let Some(exp) = introspection.exp {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            
            if now > exp {
                return Err(ServerAuthError::TokenExpired);
            }
        }
        
        // Convert to internal TokenInfo
        Ok(TokenInfo {
            active: introspection.active,
            scope: introspection.scope,
            client_id: introspection.client_id.unwrap_or_else(|| "unknown".to_string()),
            username: introspection.username,
            exp: introspection.exp,
            additional_fields: introspection.additional_fields,
        })
    }
    
    /// Validate token for specific scope
    pub async fn validate_token_scope(&self, token: &str, required_scope: &str) -> Result<TokenInfo, ServerAuthError> {
        let token_info = self.validate_token(token).await?;
        
        // Check if token has the required scope
        if let Some(scope) = &token_info.scope {
            let scopes: Vec<&str> = scope.split(' ').collect();
            if scopes.contains(&required_scope) {
                Ok(token_info)
            } else {
                Err(ServerAuthError::InsufficientScope(format!(
                    "Token does not have required scope: {}", required_scope
                )))
            }
        } else {
            Err(ServerAuthError::InsufficientScope(format!(
                "Token does not specify any scopes, required: {}", required_scope
            )))
        }
    }
    
    /// Extract token from Authorization header
    pub fn extract_token_from_header(&self, auth_header: Option<&str>) -> Result<String, ServerAuthError> {
        match auth_header {
            Some(header) => {
                if header.starts_with("Bearer ") {
                    Ok(header[7..].to_string())
                } else {
                    Err(ServerAuthError::InvalidToken(
                        "Authorization header must use Bearer scheme".to_string()
                    ))
                }
            }
            None => Err(ServerAuthError::InvalidToken("Missing Authorization header".to_string())),
        }
    }
    
    /// Register a token (for testing or custom token creation)
    pub async fn register_token(&self, token: String, token_info: TokenInfo) -> Result<(), ServerAuthError> {
        self.active_tokens.write().await.insert(token, token_info);
        Ok(())
    }
    
    /// Revoke a token
    pub async fn revoke_token(&self, token: &str) -> Result<(), ServerAuthError> {
        // If third-party auth is configured and has revocation endpoint, use that
        if let Some(config) = &self.third_party_config {
            if let Some(revocation_endpoint) = &config.revocation_endpoint {
                // Attempt to revoke with third-party
                let form = [
                    ("token", token),
                    ("token_type_hint", "access_token"),
                ];
                
                let response = self.http_client
                    .post(revocation_endpoint)
                    .basic_auth(&config.client_id, Some(&config.client_secret))
                    .form(&form)
                    .send()
                    .await
                    .map_err(|e| ServerAuthError::HttpError(e))?;
                
                if !response.status().is_success() {
                    let status = response.status();
                    let error_text = response.text().await.unwrap_or_else(|_| "Unknown error".to_string());
                    
                    // Log the error but don't fail - per OAuth spec, token revocation
                    // should succeed even for unknown tokens
                    error!("Third-party token revocation failed: HTTP {} - {}", status, error_text);
                }
            }
        }
        
        // Always remove from local storage
        let mut active_tokens = self.active_tokens.write().await;
        active_tokens.remove(token);
        
        Ok(())
    }
}

/// Extension trait for axum handlers
#[cfg(feature = "axum")]
pub mod axum_ext {
    use super::*;
    use axum::{
        extract::State,
        http::{HeaderMap, Request, StatusCode},
        middleware::Next,
        response::{IntoResponse, Response},
        Json,
    };
    
    pub async fn oauth_metadata_handler<S>(
        State(server): State<Arc<AuthServer>>,
    ) -> impl IntoResponse {
        let metadata = server.generate_metadata();
        Json(metadata)
    }
    
    pub async fn client_registration_handler<S>(
        State(server): State<Arc<AuthServer>>,
        Json(request): Json<ClientRegistrationRequest>,
    ) -> Result<impl IntoResponse, impl IntoResponse> {
        match server.handle_registration(request).await {
            Ok(response) => Ok((StatusCode::CREATED, Json(response))),
            Err(e) => Err((StatusCode::BAD_REQUEST, e.to_string())),
        }
    }
    
    pub async fn auth_middleware<B>(
        State(server): State<Arc<AuthServer>>,
        headers: HeaderMap,
        request: Request<B>,
        next: Next<B>,
    ) -> Response {
        // Extract token from Authorization header
        let auth_header = headers.get(AUTHORIZATION).and_then(|h| h.to_str().ok());
        
        match server.extract_token_from_header(auth_header) {
            Ok(token) => {
                match server.validate_token(&token).await {
                    Ok(_) => next.run(request).await,
                    Err(e) => {
                        let status = match e {
                            ServerAuthError::TokenExpired => StatusCode::UNAUTHORIZED,
                            ServerAuthError::InvalidToken(_) => StatusCode::UNAUTHORIZED,
                            ServerAuthError::InsufficientScope(_) => StatusCode::FORBIDDEN,
                            ServerAuthError::ThirdPartyAuthError(_) => StatusCode::UNAUTHORIZED,
                            _ => StatusCode::INTERNAL_SERVER_ERROR,
                        };
                        
                        (status, e.to_string()).into_response()
                    }
                }
            }
            Err(e) => (StatusCode::UNAUTHORIZED, e.to_string()).into_response(),
        }
    }
    
    /// Middleware for requiring specific scopes
    pub fn scope_middleware(scope: &'static str) -> impl Fn(
        State<Arc<AuthServer>>,
        HeaderMap,
        Request<axum::body::Body>,
        Next<axum::body::Body>,
    ) -> std::pin::Pin<Box<dyn Future<Output = Response> + Send>> + Clone {
        move |State(server): State<Arc<AuthServer>>,
              headers: HeaderMap,
              request: Request<axum::body::Body>,
              next: Next<axum::body::Body>| {
            let server = server.clone();
            Box::pin(async move {
                // Extract token from Authorization header
                let auth_header = headers.get(AUTHORIZATION).and_then(|h| h.to_str().ok());
                
                match server.extract_token_from_header(auth_header) {
                    Ok(token) => {
                        match server.validate_token_scope(&token, scope).await {
                            Ok(_) => next.run(request).await,
                            Err(e) => {
                                let status = match e {
                                    ServerAuthError::TokenExpired => StatusCode::UNAUTHORIZED,
                                    ServerAuthError::InvalidToken(_) => StatusCode::UNAUTHORIZED,
                                    ServerAuthError::InsufficientScope(_) => StatusCode::FORBIDDEN,
                                    ServerAuthError::ThirdPartyAuthError(_) => StatusCode::UNAUTHORIZED,
                                    _ => StatusCode::INTERNAL_SERVER_ERROR,
                                };
                                
                                (status, e.to_string()).into_response()
                            }
                        }
                    }
                    Err(e) => (StatusCode::UNAUTHORIZED, e.to_string()).into_response(),
                }
            })
        }
    }
}

/// Example server setup with axum
#[cfg(feature = "axum")]
pub mod example {
    use super::*;
    use axum::{
        routing::{get, post},
        Router,
    };
    
    pub async fn create_oauth_router(server: Arc<AuthServer>) -> Router {
        Router::new()
            .route("/.well-known/oauth-authorization-server", get(axum_ext::oauth_metadata_handler))
            .route("/register", post(axum_ext::client_registration_handler))
            .with_state(server)
    }
    
    /// Create an OAuth server with third-party authorization
    pub fn create_third_party_oauth_server(base_url: &str) -> Result<AuthServer, ServerAuthError> {
        let server_base_url = Url::parse(base_url)?;
        
        // Local server config (will delegate to third-party)
        let config = AuthServerConfig {
            client_id: "mcp_server".to_string(),
            client_secret: "mcp_server_secret".to_string(),
            authorize_endpoint: format!("{}/authorize", server_base_url),
            token_endpoint: format!("{}/token", server_base_url),
            registration_endpoint: format!("{}/register", server_base_url),
            issuer: server_base_url.to_string(),
            supported_scopes: vec!["mcp".to_string(), "profile".to_string()],
        };
        
        // Third-party auth server config
        let third_party_config = ThirdPartyAuthConfig {
            client_id: "mcp_client_at_third_party".to_string(),
            client_secret: "mcp_client_secret_at_third_party".to_string(),
            auth_server_url: "https://auth.example.com".to_string(),
            authorize_endpoint: "https://auth.example.com/oauth2/authorize".to_string(),
            token_endpoint: "https://auth.example.com/oauth2/token".to_string(),
            introspection_endpoint: Some("https://auth.example.com/oauth2/introspect".to_string()),
            revocation_endpoint: Some("https://auth.example.com/oauth2/revoke".to_string()),
            supported_scopes: vec!["openid".to_string(), "profile".to_string(), "mcp".to_string()],
            additional_params: HashMap::new(),
        };
        
        Ok(AuthServer::with_third_party_auth(config, third_party_config, server_base_url))
    }
    
    /// Create a router with protected resources requiring specific scopes
    pub fn create_protected_router(server: Arc<AuthServer>) -> Router {
        Router::new()
            // Path that requires any valid token
            .route("/api/resource", get(|| async { "Protected resource" }))
            .route_layer(axum::middleware::from_fn_with_state(
                server.clone(),
                axum_ext::auth_middleware,
            ))
            // Path that requires specific scope "mcp"
            .route("/api/mcp", get(|| async { "MCP protected resource" }))
            .route_layer(axum::middleware::from_fn_with_state(
                server.clone(),
                axum_ext::scope_middleware("mcp"),
            ))
            // Path that requires specific scope "profile"
            .route("/api/profile", get(|| async { "Profile protected resource" }))
            .route_layer(axum::middleware::from_fn_with_state(
                server.clone(),
                axum_ext::scope_middleware("profile"),
            ))
    }
} 