use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use oauth2::{
    AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken, EmptyExtraTokenFields,
    PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, RefreshToken, Scope, StandardTokenResponse,
    TokenResponse, TokenUrl,
    basic::{BasicClient, BasicTokenType},
};
use reqwest::{Client as HttpClient, IntoUrl, StatusCode, Url, header::AUTHORIZATION};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::RwLock;
use tracing::{debug, error};

/// Auth error
#[derive(Debug, Error)]
pub enum AuthError {
    #[error("OAuth authorization required")]
    AuthorizationRequired,

    #[error("OAuth authorization failed: {0}")]
    AuthorizationFailed(String),

    #[error("OAuth token exchange failed: {0}")]
    TokenExchangeFailed(String),

    #[error("OAuth token refresh failed: {0}")]
    TokenRefreshFailed(String),

    #[error("HTTP error: {0}")]
    HttpError(#[from] reqwest::Error),

    #[error("OAuth error: {0}")]
    OAuthError(String),

    #[error("Metadata error: {0}")]
    MetadataError(String),

    #[error("URL parse error: {0}")]
    UrlError(#[from] url::ParseError),

    #[error("No authorization support detected")]
    NoAuthorizationSupport,

    #[error("Internal error: {0}")]
    InternalError(String),

    #[error("Invalid token type: {0}")]
    InvalidTokenType(String),

    #[error("Token expired")]
    TokenExpired,

    #[error("Invalid scope: {0}")]
    InvalidScope(String),

    #[error("Registration failed: {0}")]
    RegistrationFailed(String),
}

/// oauth2 metadata
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AuthorizationMetadata {
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub registration_endpoint: String,
    pub issuer: Option<String>,
    pub jwks_uri: Option<String>,
    pub scopes_supported: Option<Vec<String>>,
}

/// oauth2 client config
#[derive(Debug, Clone)]
pub struct OAuthClientConfig {
    pub client_id: String,
    pub client_secret: Option<String>,
    pub scopes: Vec<String>,
    pub redirect_uri: String,
}

// add type aliases for oauth2 types
type OAuthErrorResponse = oauth2::StandardErrorResponse<oauth2::basic::BasicErrorResponseType>;
type OAuthTokenResponse = StandardTokenResponse<EmptyExtraTokenFields, BasicTokenType>;
type OAuthTokenIntrospection =
    oauth2::StandardTokenIntrospectionResponse<EmptyExtraTokenFields, BasicTokenType>;
type OAuthRevocableToken = oauth2::StandardRevocableToken;
type OAuthRevocationError = oauth2::StandardErrorResponse<oauth2::RevocationErrorResponseType>;
type OAuthClient = oauth2::Client<
    OAuthErrorResponse,
    OAuthTokenResponse,
    OAuthTokenIntrospection,
    OAuthRevocableToken,
    OAuthRevocationError,
    oauth2::EndpointSet,
    oauth2::EndpointNotSet,
    oauth2::EndpointNotSet,
    oauth2::EndpointNotSet,
    oauth2::EndpointSet,
>;

/// oauth2 auth manager
pub struct AuthorizationManager {
    http_client: HttpClient,
    metadata: Option<AuthorizationMetadata>,
    oauth_client: Option<OAuthClient>,
    credentials: RwLock<Option<OAuthTokenResponse>>,
    pkce_verifier: RwLock<Option<PkceCodeVerifier>>,
    expires_at: RwLock<Option<Instant>>,
    base_url: Url,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientRegistrationRequest {
    pub client_name: String,
    pub redirect_uris: Vec<String>,
    pub grant_types: Vec<String>,
    pub token_endpoint_auth_method: String,
    pub response_types: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientRegistrationResponse {
    pub client_id: String,
    pub client_secret: String,
    pub client_name: String,
    pub redirect_uris: Vec<String>,
}

impl AuthorizationManager {
    /// create new auth manager with base url
    pub async fn new<U: IntoUrl>(base_url: U) -> Result<Self, AuthError> {
        let base_url = base_url.into_url()?;
        let http_client = HttpClient::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| AuthError::InternalError(e.to_string()))?;

        let manager = Self {
            http_client,
            metadata: None,
            oauth_client: None,
            credentials: RwLock::new(None),
            pkce_verifier: RwLock::new(None),
            expires_at: RwLock::new(None),
            base_url,
        };

        Ok(manager)
    }

    pub fn with_client(&mut self, http_client: HttpClient) -> Result<(), AuthError> {
        self.http_client = http_client;
        Ok(())
    }

    /// discover oauth2 metadata
    pub async fn discover_metadata(&self) -> Result<AuthorizationMetadata, AuthError> {
        // according to the specification, the metadata should be located at "/.well-known/oauth-authorization-server"
        let mut discovery_url = self.base_url.clone();
        discovery_url.set_path("/.well-known/oauth-authorization-server");
        debug!("discovery url: {:?}", discovery_url);
        let response = self
            .http_client
            .get(discovery_url)
            .header("MCP-Protocol-Version", "2024-11-05")
            .send()
            .await?;

        if response.status() == StatusCode::OK {
            let metadata = response
                .json::<AuthorizationMetadata>()
                .await
                .map_err(|e| {
                    AuthError::MetadataError(format!("Failed to parse metadata: {}", e))
                })?;
            debug!("metadata: {:?}", metadata);
            Ok(metadata)
        } else {
            // fallback to default endpoints
            let mut auth_base = self.base_url.clone();
            // discard the path part, only keep scheme, host, port
            auth_base.set_path("");

            Ok(AuthorizationMetadata {
                authorization_endpoint: format!("{}/authorize", auth_base),
                token_endpoint: format!("{}/token", auth_base),
                registration_endpoint: format!("{}/register", auth_base),
                issuer: None,
                jwks_uri: None,
                scopes_supported: None,
            })
        }
    }

    /// configure oauth2 client with client credentials
    pub fn configure_client(&mut self, config: OAuthClientConfig) -> Result<(), AuthError> {
        if self.metadata.is_none() {
            return Err(AuthError::NoAuthorizationSupport);
        }

        let metadata = self.metadata.as_ref().unwrap();

        let auth_url = AuthUrl::new(metadata.authorization_endpoint.clone())
            .map_err(|e| AuthError::OAuthError(format!("Invalid authorization URL: {}", e)))?;

        let token_url = TokenUrl::new(metadata.token_endpoint.clone())
            .map_err(|e| AuthError::OAuthError(format!("Invalid token URL: {}", e)))?;

        // debug!("token url: {:?}", token_url);
        let client_id = ClientId::new(config.client_id);
        let redirect_url = RedirectUrl::new(config.redirect_uri.clone())
            .map_err(|e| AuthError::OAuthError(format!("Invalid re URL: {}", e)))?;

        debug!("client_id: {:?}", client_id);
        let mut client_builder = BasicClient::new(client_id.clone())
            .set_auth_uri(auth_url)
            .set_token_uri(token_url)
            .set_redirect_uri(redirect_url);

        if let Some(secret) = config.client_secret {
            client_builder = client_builder.set_client_secret(ClientSecret::new(secret));
        }

        self.oauth_client = Some(client_builder);
        Ok(())
    }

    /// dynamic register oauth2 client
    pub async fn register_client(
        &mut self,
        name: &str,
        redirect_uri: &str,
    ) -> Result<OAuthClientConfig, AuthError> {
        if self.metadata.is_none() {
            error!("No authorization support detected");
            return Err(AuthError::NoAuthorizationSupport);
        }

        let metadata = self.metadata.as_ref().unwrap();
        let registration_url = metadata.registration_endpoint.clone();

        debug!("registration url: {:?}", registration_url);
        // prepare registration request
        let registration_request = ClientRegistrationRequest {
            client_name: name.to_string(),
            redirect_uris: vec![redirect_uri.to_string()],
            grant_types: vec![
                "authorization_code".to_string(),
                "refresh_token".to_string(),
            ],
            token_endpoint_auth_method: "none".to_string(), // public client
            response_types: vec!["code".to_string()],
        };

        debug!("registration request: {:?}", registration_request);

        let response = match self
            .http_client
            .post(registration_url)
            .json(&registration_request)
            .send()
            .await
        {
            Ok(response) => response,
            Err(e) => {
                error!("Registration request failed: {}", e);
                return Err(AuthError::RegistrationFailed(format!(
                    "HTTP request error: {}",
                    e
                )));
            }
        };

        if !response.status().is_success() {
            let status = response.status();
            let error_text = match response.text().await {
                Ok(text) => text,
                Err(_) => "cannot get error details".to_string(),
            };

            error!("Registration failed: HTTP {} - {}", status, error_text);
            return Err(AuthError::RegistrationFailed(format!(
                "HTTP {}: {}",
                status, error_text
            )));
        }
        debug!("registration response: {:?}", response);
        let reg_response = match response.json::<ClientRegistrationResponse>().await {
            Ok(response) => response,
            Err(e) => {
                error!("Failed to parse registration response: {}", e);
                return Err(AuthError::RegistrationFailed(format!(
                    "analyze response error: {}",
                    e
                )));
            }
        };

        let config = OAuthClientConfig {
            client_id: reg_response.client_id,
            client_secret: Some(reg_response.client_secret),
            redirect_uri: redirect_uri.to_string(),
            scopes: vec![],
        };

        self.configure_client(config.clone())?;
        Ok(config)
    }

    /// generate authorization url
    pub async fn get_authorization_url(&self, scopes: &[&str]) -> Result<String, AuthError> {
        let oauth_client = self
            .oauth_client
            .as_ref()
            .ok_or_else(|| AuthError::InternalError("OAuth client not configured".to_string()))?;

        // generate pkce challenge
        let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();

        // build authorization request
        let mut auth_request = oauth_client
            .authorize_url(CsrfToken::new_random)
            .set_pkce_challenge(pkce_challenge);

        // add request scopes
        for scope in scopes {
            auth_request = auth_request.add_scope(Scope::new(scope.to_string()));
        }

        let (auth_url, _csrf_token) = auth_request.url();

        // store pkce verifier for later use
        *self.pkce_verifier.write().await = Some(pkce_verifier);
        debug!("set pkce verifier: {:?}", self.pkce_verifier.read().await);

        Ok(auth_url.to_string())
    }

    /// exchange authorization code for access token
    pub async fn exchange_code_for_token(
        &self,
        code: &str,
    ) -> Result<StandardTokenResponse<EmptyExtraTokenFields, BasicTokenType>, AuthError> {
        debug!("start exchange code for token: {:?}", code);
        let oauth_client = self
            .oauth_client
            .as_ref()
            .ok_or_else(|| AuthError::InternalError("OAuth client not configured".to_string()))?;

        let pkce_verifier = self
            .pkce_verifier
            .write()
            .await
            .take()
            .ok_or_else(|| AuthError::InternalError("PKCE verifier not found".to_string()))?;

        let http_client = reqwest::ClientBuilder::new()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| AuthError::InternalError(e.to_string()))?;
        debug!("client_id: {:?}", oauth_client.client_id());

        // exchange token
        let token_result = oauth_client
            .exchange_code(AuthorizationCode::new(code.to_string()))
            .add_extra_param("client_id", oauth_client.client_id().to_string())
            .set_pkce_verifier(pkce_verifier)
            .request_async(&http_client)
            .await
            .map_err(|e| AuthError::TokenExchangeFailed(e.to_string()))?;

        // get expires_in from token response
        let expires_in = token_result.expires_in();
        if let Some(expires_in) = expires_in {
            let expires_at = Instant::now() + expires_in;
            *self.expires_at.write().await = Some(expires_at);
        }
        debug!("exchange token result: {:?}", token_result);
        // store credentials
        *self.credentials.write().await = Some(token_result.clone());

        Ok(token_result)
    }

    /// get access token, if expired, refresh it automatically
    pub async fn get_access_token(&self) -> Result<String, AuthError> {
        let credentials = self.credentials.read().await;

        if let Some(creds) = credentials.as_ref() {
            // check if the token is expire
            if let Some(expires_at) = *self.expires_at.read().await {
                if expires_at < Instant::now() {
                    // token expired, try to refresh , release the lock
                    drop(credentials);
                    let new_creds = self.refresh_token().await?;
                    return Ok(new_creds.access_token().secret().to_string());
                }
            }

            Ok(creds.access_token().secret().to_string())
        } else {
            Err(AuthError::AuthorizationRequired)
        }
    }

    /// refresh access token
    pub async fn refresh_token(
        &self,
    ) -> Result<StandardTokenResponse<EmptyExtraTokenFields, BasicTokenType>, AuthError> {
        let oauth_client = self
            .oauth_client
            .as_ref()
            .ok_or_else(|| AuthError::InternalError("OAuth client not configured".to_string()))?;

        let current_credentials = self
            .credentials
            .read()
            .await
            .clone()
            .ok_or_else(|| AuthError::AuthorizationRequired)?;

        let refresh_token = current_credentials.refresh_token().ok_or_else(|| {
            AuthError::TokenRefreshFailed("No refresh token available".to_string())
        })?;
        debug!("refresh token: {:?}", refresh_token);
        // refresh token
        let token_result = oauth_client
            .exchange_refresh_token(&RefreshToken::new(refresh_token.secret().to_string()))
            .request_async(&self.http_client)
            .await
            .map_err(|e| AuthError::TokenRefreshFailed(e.to_string()))?;

        // store new credentials
        *self.credentials.write().await = Some(token_result.clone());

        // get expires_in from token response
        let expires_in = token_result.expires_in();
        if let Some(expires_in) = expires_in {
            let expires_at = Instant::now() + expires_in;
            *self.expires_at.write().await = Some(expires_at);
        }
        Ok(token_result)
    }

    /// prepare request, add authorization header
    pub async fn prepare_request(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<reqwest::RequestBuilder, AuthError> {
        let token = self.get_access_token().await?;
        Ok(request.header(AUTHORIZATION, format!("Bearer {}", token)))
    }

    /// handle response, check if need to re-authorize
    pub async fn handle_response(
        &self,
        response: reqwest::Response,
    ) -> Result<reqwest::Response, AuthError> {
        if response.status() == StatusCode::UNAUTHORIZED {
            // 401 Unauthorized, need to re-authorize
            Err(AuthError::AuthorizationRequired)
        } else {
            Ok(response)
        }
    }
}

/// oauth2 authorization session, for guiding user to complete the authorization process
pub struct AuthorizationSession {
    pub auth_manager: AuthorizationManager,
    pub auth_url: String,
    pub redirect_uri: String,
}

impl AuthorizationSession {
    /// create new authorization session
    pub async fn new(
        mut auth_manager: AuthorizationManager,
        scopes: &[&str],
        redirect_uri: &str,
    ) -> Result<Self, AuthError> {
        // set redirect uri
        let config = OAuthClientConfig {
            client_id: "mcp-client".to_string(), // temporary id, will be updated by dynamic registration
            client_secret: None,
            scopes: scopes.iter().map(|s| s.to_string()).collect(),
            redirect_uri: redirect_uri.to_string(),
        };

        // try to dynamic register client
        let config = match auth_manager
            .register_client("MCP Client", redirect_uri)
            .await
        {
            Ok(config) => config,
            Err(e) => {
                eprintln!("Dynamic registration failed: {}", e);
                // fallback to default config
                config
            }
        };
        // reset client config
        auth_manager.configure_client(config)?;
        let auth_url = auth_manager.get_authorization_url(scopes).await?;

        Ok(Self {
            auth_manager,
            auth_url,
            redirect_uri: redirect_uri.to_string(),
        })
    }

    /// get authorization url
    pub fn get_authorization_url(&self) -> &str {
        &self.auth_url
    }

    /// handle authorization code callback
    pub async fn handle_callback(
        &self,
        code: &str,
    ) -> Result<StandardTokenResponse<EmptyExtraTokenFields, BasicTokenType>, AuthError> {
        self.auth_manager.exchange_code_for_token(code).await
    }
}

/// http client extension, automatically add authorization header
pub struct AuthorizedHttpClient {
    auth_manager: Arc<AuthorizationManager>,
    inner_client: HttpClient,
}

impl AuthorizedHttpClient {
    /// create new authorized http client
    pub fn new(auth_manager: Arc<AuthorizationManager>, client: Option<HttpClient>) -> Self {
        let inner_client = client.unwrap_or_default();
        Self {
            auth_manager,
            inner_client,
        }
    }

    /// send authorized request
    pub async fn request<U: IntoUrl>(
        &self,
        method: reqwest::Method,
        url: U,
    ) -> Result<reqwest::RequestBuilder, AuthError> {
        let request = self.inner_client.request(method, url);
        self.auth_manager.prepare_request(request).await
    }

    /// send get request
    pub async fn get<U: IntoUrl>(&self, url: U) -> Result<reqwest::Response, AuthError> {
        let request = self.request(reqwest::Method::GET, url).await?;
        let response = request.send().await?;
        self.auth_manager.handle_response(response).await
    }

    /// send post request
    pub async fn post<U: IntoUrl>(&self, url: U) -> Result<reqwest::RequestBuilder, AuthError> {
        self.request(reqwest::Method::POST, url).await
    }
}

/// OAuth state machine
/// Use the OAuthState to manage the OAuth client is more recommend
/// But also you can use the AuthorizationManager,AuthorizationSession,AuthorizedHttpClient directly
pub enum OAuthState {
    /// the AuthorizationManager
    Unauthorized(AuthorizationManager),
    /// the AuthorizationSession
    Session(AuthorizationSession),
    /// the authd AuthorizationManager
    Authorized(AuthorizationManager),
    /// the authd http client
    AuthorizedHttpClient(AuthorizedHttpClient),
}

impl OAuthState {
    /// Create new OAuth state machine
    pub async fn new<U: IntoUrl>(
        base_url: U,
        client: Option<HttpClient>,
    ) -> Result<Self, AuthError> {
        let mut manager = AuthorizationManager::new(base_url).await?;
        if let Some(client) = client {
            manager.with_client(client)?;
        }
        Ok(OAuthState::Unauthorized(manager))
    }

    /// start authorization
    pub async fn start_authorization(
        &mut self,
        scopes: &[&str],
        redirect_uri: &str,
    ) -> Result<(), AuthError> {
        if let OAuthState::Unauthorized(mut manager) = std::mem::replace(
            self,
            OAuthState::Unauthorized(AuthorizationManager::new("http://localhost").await?),
        ) {
            debug!("start discovery");
            let metadata = manager.discover_metadata().await?;
            manager.metadata = Some(metadata);
            debug!("start session");
            let session = AuthorizationSession::new(manager, scopes, redirect_uri).await?;
            *self = OAuthState::Session(session);
            Ok(())
        } else {
            Err(AuthError::InternalError(
                "Already in session state".to_string(),
            ))
        }
    }

    /// complete authorization
    pub async fn complete_authorization(&mut self) -> Result<(), AuthError> {
        if let OAuthState::Session(session) = std::mem::replace(
            self,
            OAuthState::Unauthorized(AuthorizationManager::new("http://localhost").await?),
        ) {
            *self = OAuthState::Authorized(session.auth_manager);
            Ok(())
        } else {
            Err(AuthError::InternalError("Not in session state".to_string()))
        }
    }
    /// covert to authorized http client
    pub async fn to_authorized_http_client(&mut self) -> Result<(), AuthError> {
        if let OAuthState::Authorized(manager) = std::mem::replace(
            self,
            OAuthState::Authorized(AuthorizationManager::new("http://localhost").await?),
        ) {
            *self = OAuthState::AuthorizedHttpClient(AuthorizedHttpClient::new(
                Arc::new(manager),
                None,
            ));
            Ok(())
        } else {
            Err(AuthError::InternalError(
                "Not in authorized state".to_string(),
            ))
        }
    }
    /// get current authorization url
    pub async fn get_authorization_url(&self) -> Result<String, AuthError> {
        match self {
            OAuthState::Session(session) => Ok(session.get_authorization_url().to_string()),
            OAuthState::Unauthorized(_) => {
                Err(AuthError::InternalError("Not in session state".to_string()))
            }
            OAuthState::Authorized(_) => {
                Err(AuthError::InternalError("Already authorized".to_string()))
            }
            OAuthState::AuthorizedHttpClient(_) => {
                Err(AuthError::InternalError("Already authorized".to_string()))
            }
        }
    }

    /// handle authorization callback
    pub async fn handle_callback(&mut self, code: &str) -> Result<(), AuthError> {
        match self {
            OAuthState::Session(session) => {
                session.handle_callback(code).await?;
                self.complete_authorization().await
            }
            OAuthState::Unauthorized(_) => {
                Err(AuthError::InternalError("Not in session state".to_string()))
            }
            OAuthState::Authorized(_) => {
                Err(AuthError::InternalError("Already authorized".to_string()))
            }
            OAuthState::AuthorizedHttpClient(_) => {
                Err(AuthError::InternalError("Already authorized".to_string()))
            }
        }
    }

    /// get access token
    pub async fn get_access_token(&self) -> Result<String, AuthError> {
        match self {
            OAuthState::Unauthorized(manager) => manager.get_access_token().await,
            OAuthState::Session(_) => {
                Err(AuthError::InternalError("Not in manager state".to_string()))
            }
            OAuthState::Authorized(_) => {
                Err(AuthError::InternalError("Already authorized".to_string()))
            }
            OAuthState::AuthorizedHttpClient(_) => {
                Err(AuthError::InternalError("Already authorized".to_string()))
            }
        }
    }

    /// refresh access token
    pub async fn refresh_token(&self) -> Result<(), AuthError> {
        match self {
            OAuthState::Unauthorized(_) => {
                Err(AuthError::InternalError("Not in manager state".to_string()))
            }
            OAuthState::Session(_) => {
                Err(AuthError::InternalError("Not in manager state".to_string()))
            }
            OAuthState::Authorized(manager) => {
                manager.refresh_token().await?;
                Ok(())
            }
            OAuthState::AuthorizedHttpClient(_) => {
                Err(AuthError::InternalError("Already authorized".to_string()))
            }
        }
    }
}
