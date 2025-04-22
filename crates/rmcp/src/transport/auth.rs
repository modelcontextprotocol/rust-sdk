use std::{sync::Arc, time::Duration};

use oauth2::{
    AccessToken, AuthUrl, AuthorizationCode, AuthorizationRequest, ClientId, ClientSecret,
    CsrfToken, EmptyExtraTokenFields, PkceCodeChallenge, PkceCodeVerifier, RedirectUrl,
    RefreshToken, RefreshTokenRequest, Scope, StandardTokenResponse, TokenResponse, TokenType,
    TokenUrl,
    basic::{BasicClient, BasicTokenType},
    reqwest::http_client,
};
use reqwest::{Client as HttpClient, IntoUrl, StatusCode, Url, header::AUTHORIZATION};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::{
    sync::{Mutex, RwLock},
    time::{self, Instant},
};
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

/// oauth2 auth manager
pub struct AuthorizationManager {
    http_client: HttpClient,
    metadata: Option<AuthorizationMetadata>,
    oauth_client: Option<BasicClient>,
    credentials: RwLock<Option<StandardTokenResponse<EmptyExtraTokenFields, BasicTokenType>>>,
    pkce_verifier: RwLock<Option<PkceCodeVerifier>>,
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
    /// create new auth manager
    pub async fn new<U: IntoUrl>(base_url: U) -> Result<Self, AuthError> {
        let base_url = base_url.into_url()?;
        let http_client = HttpClient::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| AuthError::InternalError(e.to_string()))?;

        let mut manager = Self {
            http_client,
            metadata: None,
            oauth_client: None,
            credentials: RwLock::new(None),
            pkce_verifier: RwLock::new(None),
            base_url,
        };

        // try to discover oauth2 metadata
        if let Ok(metadata) = manager.discover_metadata().await {
            manager.metadata = Some(metadata);
        }

        Ok(manager)
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

        let client_id = ClientId::new(config.client_id);
        let redirect_url = RedirectUrl::new(config.redirect_uri.clone())
            .map_err(|e| AuthError::OAuthError(format!("Invalid registry URL: {}", e)))?;

        let mut client_builder = BasicClient::new(
            client_id.clone(),
            None,
            auth_url.clone(),
            Some(token_url.clone()),
        )
        .set_redirect_uri(redirect_url.clone());

        if let Some(secret) = config.client_secret {
            client_builder = BasicClient::new(
                client_id,
                Some(ClientSecret::new(secret)),
                auth_url,
                Some(token_url),
            )
            .set_redirect_uri(redirect_url);
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

        Ok(auth_url.to_string())
    }

    /// exchange authorization code for access token
    pub async fn exchange_code_for_token(
        &self,
        code: &str,
    ) -> Result<StandardTokenResponse<EmptyExtraTokenFields, BasicTokenType>, AuthError> {
        let oauth_client = self
            .oauth_client
            .as_ref()
            .ok_or_else(|| AuthError::InternalError("OAuth client not configured".to_string()))?;

        let pkce_verifier = self.pkce_verifier.write().await.take().unwrap();

        // exchange token
        let token_result = oauth_client
            .exchange_code(AuthorizationCode::new(code.to_string()))
            .set_pkce_verifier(pkce_verifier)
            .request(http_client)
            .map_err(|e| AuthError::TokenExchangeFailed(e.to_string()))?;

        // store credentials
        *self.credentials.write().await = Some(token_result.clone());

        Ok(token_result)
    }

    /// get access token, if expired, refresh it automatically
    pub async fn get_access_token(&self) -> Result<String, AuthError> {
        let credentials = self.credentials.read().await;

        if let Some(creds) = credentials.as_ref() {
            // check if the token is expired
            if let Some(expires_in) = creds.expires_in() {
                if expires_in <= Duration::from_secs(0) {
                    // token expired, try to refresh
                    drop(credentials); // release the lock
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

        // refresh token
        let token_result = oauth_client
            .exchange_refresh_token(&RefreshToken::new(refresh_token.secret().to_string()))
            .request(http_client)
            .map_err(|e| AuthError::TokenRefreshFailed(e.to_string()))?;

        // store new credentials
        *self.credentials.write().await = Some(token_result.clone());

        Ok(token_result)
    }

    /// prepare request, add authorization header
    pub async fn prepare_request(
        &self,
        mut request: reqwest::RequestBuilder,
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
    pub auth_manager: Arc<Mutex<AuthorizationManager>>,
    pub auth_url: String,
    pub redirect_uri: String,
    pub pkce_verifier: PkceCodeVerifier,
}

impl AuthorizationSession {
    /// create new authorization session
    pub async fn new(
        auth_manager: Arc<Mutex<AuthorizationManager>>,
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
            .lock()
            .await
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
        auth_manager.lock().await.configure_client(config)?;
        let auth_url = auth_manager
            .lock()
            .await
            .get_authorization_url(scopes)
            .await?;
        let pkce_verifier = auth_manager
            .lock()
            .await
            .pkce_verifier
            .write()
            .await
            .take()
            .unwrap();
        Ok(Self {
            auth_manager,
            auth_url,
            redirect_uri: redirect_uri.to_string(),
            pkce_verifier,
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
        self.auth_manager
            .lock()
            .await
            .exchange_code_for_token(code)
            .await
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
        let inner_client = client.unwrap_or_else(|| HttpClient::new());
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
