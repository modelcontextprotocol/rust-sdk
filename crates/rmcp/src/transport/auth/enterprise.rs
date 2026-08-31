//! Non-interactive enterprise-managed authorization (EMA/XAA) token exchanges.
//!
//! Exchanges an enterprise refresh token for an ID-JAG, then for an MCP access
//! token. Callers must discover and approve both servers and their registrations
//! before supplying a credential. This module does not discover servers, log in,
//! persist credentials, or decide when to reauthenticate.
//!
//! Configure each pre-registered client's approved authentication method separately:
//! HTTP Basic, a client secret in the request body, or a freshly signed JWT client
//! assertion. Public-client authentication requires the server's explicit approval.
//! This helper requires one MCP resource and does not implement Rich
//! Authorization Requests or DPoP. Redemption consumes the SDK's ID-JAG handle,
//! without automatic retries; server-side replay policy remains the server's responsibility.
//!
//! ID-JAG checks below enforce structure and claim bindings, not cryptographic
//! signature verification. Assertions come directly from the trusted IdP token
//! endpoint; the resource authorization server must verify their signatures.
//!
//! ```no_run
//! use oauth2::{ClientSecret, RefreshToken};
//! use rmcp::transport::auth::{default_oauth_http_client, enterprise::*};
//!
//! # async fn authorize(refresh: &RefreshToken, idp_secret: ClientSecret, resource_secret: ClientSecret) -> Result<(), Box<dyn std::error::Error>> {
//! // Enable `auth-enterprise-managed` and a TLS feature such as `reqwest`.
//! let http = default_oauth_http_client()?;
//! let token = EmaExchangeRequest::new(
//!     EmaAuthorizationServer::new("https://idp.example", "https://idp.example/token", "idp-client")
//!         .with_client_authentication(EmaClientAuthentication::ClientSecretBasic(idp_secret)),
//!     EmaAuthorizationServer::new("https://as.example", "https://as.example/token", "mcp-client")
//!         .with_client_authentication(EmaClientAuthentication::ClientSecretBasic(resource_secret)),
//!     "https://mcp.example", refresh,
//! ).with_scopes(["files.read"]).exchange(&http, &http).await?;
//! // Use token.access_token only for the approved MCP resource; never log it.
//! // token.scopes contains the final granted scopes, which may be narrower.
//! # Ok(()) }
//! ```

use std::{
    collections::HashSet,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use oauth2::{AccessToken, ClientSecret, RefreshToken};
use serde::{Deserialize, de::DeserializeOwned};
use thiserror::Error;
use url::{Host, Url};

use super::{
    DEFAULT_HTTP_TIMEOUT, MAX_OAUTH_HTTP_RESPONSE_BODY_BYTES, OAuthHttpClient,
    OAuthHttpRedirectPolicy, OAuthHttpRequest,
};

const ID_JAG_TOKEN_TYPE: &str = "urn:ietf:params:oauth:token-type:id-jag";

/// Authentication approved for a pre-registered client at one authorization server.
///
/// The selected method is used as configured, without negotiation or fallback.
#[derive(Clone)]
#[non_exhaustive]
pub enum EmaClientAuthentication {
    /// Public client (`token_endpoint_auth_method=none`), only if the server permits it.
    None,
    /// `client_secret_basic`, with OAuth form encoding before HTTP Basic encoding.
    ClientSecretBasic(ClientSecret),
    /// `client_secret_post`, for servers requiring credentials in the request body.
    ClientSecretPost(ClientSecret),
    /// Fresh JWT client assertions, such as `private_key_jwt` or `client_secret_jwt`.
    /// Signing, key custody, claims, and the registered algorithm belong to the provider.
    JwtAssertion(Arc<dyn EmaClientAssertionProvider>),
}

impl std::fmt::Debug for EmaClientAuthentication {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::None => "None",
            Self::ClientSecretBasic(_) => "ClientSecretBasic { .. }",
            Self::ClientSecretPost(_) => "ClientSecretPost { .. }",
            Self::JwtAssertion(_) => "JwtAssertion { .. }",
        })
    }
}

impl EmaClientAuthentication {
    fn validate(&self) -> Result<(), EmaError> {
        if let Self::ClientSecretBasic(secret) | Self::ClientSecretPost(secret) = self
            && secret.secret().trim().is_empty()
        {
            return Err(EmaError::InvalidRequest("client secret must not be empty"));
        }
        Ok(())
    }
}

/// A signed JWT used to authenticate the client, distinct from the ID-JAG grant.
pub struct EmaClientAssertion(String);

impl EmaClientAssertion {
    /// Wrap a fresh assertion without exposing it through `Debug`.
    pub fn new(assertion: String) -> Self {
        Self(assertion)
    }
}

impl std::fmt::Debug for EmaClientAssertion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EmaClientAssertion { .. }")
    }
}

/// Creates client assertions on demand, allowing keys to remain in an external signer.
///
/// Called once immediately before each token request, including delayed ID-JAG
/// redemption. Set `iss` and `sub` to the registered client identifier and `aud`
/// to the server's approved audience, with a short expiration and a fresh `jti`.
/// The SDK does not sign or validate these assertions. Provider failures are
/// sanitized and the call shares the token request's timeout. Cancellation may
/// occur when that deadline expires.
#[async_trait::async_trait]
pub trait EmaClientAssertionProvider: Send + Sync {
    async fn create_assertion(
        &self,
        server: &EmaAuthorizationServer,
    ) -> Result<EmaClientAssertion, Box<dyn std::error::Error + Send + Sync>>;
}

/// A trusted server with a pre-registered client and its approved authentication method.
#[derive(Clone)]
#[non_exhaustive]
pub struct EmaAuthorizationServer {
    /// Exact issuer identifier from approved metadata.
    pub issuer: String,
    /// Token endpoint from that metadata.
    pub token_endpoint: String,
    /// Pre-registered client identifier.
    pub client_id: String,
    client_authentication: EmaClientAuthentication,
}

impl EmaAuthorizationServer {
    /// Use approved metadata and a public-client registration (`token_endpoint_auth_method=none`).
    /// Set [`Self::with_client_authentication`] for a confidential client.
    pub fn new(
        issuer: impl Into<String>,
        token_endpoint: impl Into<String>,
        client_id: impl Into<String>,
    ) -> Self {
        Self {
            issuer: issuer.into(),
            token_endpoint: token_endpoint.into(),
            client_id: client_id.into(),
            client_authentication: EmaClientAuthentication::None,
        }
    }

    /// Select the authentication method approved for this server's client registration.
    pub fn with_client_authentication(mut self, authentication: EmaClientAuthentication) -> Self {
        self.client_authentication = authentication;
        self
    }
}

/// A refresh-token exchange bound to one MCP resource and two registered clients.
pub struct EmaExchangeRequest<'a> {
    idp: EmaAuthorizationServer,
    resource_as: EmaAuthorizationServer,
    resource: &'a str,
    refresh_token: &'a RefreshToken,
    scopes: Vec<String>,
}

impl<'a> EmaExchangeRequest<'a> {
    /// No scope parameter is sent until [`Self::with_scopes`] is used.
    pub fn new(
        idp: EmaAuthorizationServer,
        resource_as: EmaAuthorizationServer,
        resource: &'a str,
        refresh_token: &'a RefreshToken,
    ) -> Self {
        Self {
            idp,
            resource_as,
            resource,
            refresh_token,
            scopes: Vec::new(),
        }
    }

    /// Request distinct non-empty scope tokens; the IdP may narrow them.
    /// An empty iterator omits `scope`, rather than requesting an empty grant.
    pub fn with_scopes<I, S>(mut self, scopes: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.scopes = scopes.into_iter().map(Into::into).collect();
        self
    }

    /// Obtain an ID-JAG without redirects or retries, checking its resource/client bindings.
    /// The returned assertion does not retain the enterprise refresh token.
    pub async fn exchange_id_jag(
        self,
        idp_http: &dyn OAuthHttpClient,
    ) -> Result<EmaIdJag, EmaError> {
        for endpoint in [
            self.resource,
            &self.idp.issuer,
            &self.idp.token_endpoint,
            &self.resource_as.issuer,
            &self.resource_as.token_endpoint,
        ] {
            validate_endpoint(endpoint)?;
        }
        if self.idp.issuer == self.resource_as.issuer {
            return Err(EmaError::InvalidRequest(
                "IdP and resource AS issuers must differ",
            ));
        }
        if self.idp.client_id.trim().is_empty()
            || self.resource_as.client_id.trim().is_empty()
            || self.refresh_token.secret().trim().is_empty()
        {
            return Err(EmaError::InvalidRequest(
                "client IDs and refresh token must not be empty",
            ));
        }
        self.idp.client_authentication.validate()?;
        self.resource_as.client_authentication.validate()?;
        let requested: HashSet<&str> = self.scopes.iter().map(String::as_str).collect();
        if requested.len() != self.scopes.len() || self.scopes.iter().any(|s| !is_scope_token(s)) {
            return Err(EmaError::InvalidRequest(
                "scopes must be distinct non-empty tokens",
            ));
        }
        let mut params = vec![
            (
                "grant_type",
                "urn:ietf:params:oauth:grant-type:token-exchange",
            ),
            ("requested_token_type", ID_JAG_TOKEN_TYPE),
            (
                "subject_token_type",
                "urn:ietf:params:oauth:token-type:refresh_token",
            ),
            ("subject_token", self.refresh_token.secret()),
            ("audience", self.resource_as.issuer.as_str()),
            ("resource", self.resource),
        ];
        let scope = self.scopes.join(" ");
        if !scope.is_empty() {
            params.push(("scope", &scope));
        }
        let jag: IdJagResponse = post_form(
            idp_http,
            &self.idp,
            &params,
            EmaExchangeStage::IdentityProvider,
            None,
            unix_time,
        )
        .await?;
        let (granted, expires_at) = jag.validate(&self, &requested)?;
        Ok(EmaIdJag {
            assertion: AccessToken::new(jag.access_token),
            scopes: granted,
            resource_as: self.resource_as,
            resource: self.resource.to_owned(),
            expires_at,
        })
    }

    /// Perform both exchanges with independently routed HTTP clients and no automatic retries.
    pub async fn exchange(
        self,
        idp_http: &dyn OAuthHttpClient,
        resource_http: &dyn OAuthHttpClient,
    ) -> Result<EmaAccessToken, EmaError> {
        self.exchange_id_jag(idp_http)
            .await?
            .exchange(resource_http)
            .await
    }
}

/// An IdP-issued ID-JAG whose structure and bindings have been checked, not its signature.
pub struct EmaIdJag {
    assertion: AccessToken,
    scopes: HashSet<String>,
    resource_as: EmaAuthorizationServer,
    resource: String,
    expires_at: u64,
}

impl std::fmt::Debug for EmaAuthorizationServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EmaAuthorizationServer { .. }")
    }
}

impl std::fmt::Debug for EmaExchangeRequest<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EmaExchangeRequest { .. }")
    }
}

impl std::fmt::Debug for EmaIdJag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EmaIdJag { .. }")
    }
}

impl EmaIdJag {
    /// The assertion to present to the approved resource authorization server.
    pub fn assertion(&self) -> &AccessToken {
        &self.assertion
    }

    /// The scope tokens carried by the assertion; an empty set means scope was omitted.
    pub fn scopes(&self) -> &HashSet<String> {
        &self.scopes
    }

    /// Redeem this assertion once, at its approved resource AS, without redirects or retries.
    /// Consume the grant so this helper cannot accidentally replay it after a failed exchange.
    pub async fn exchange(self, http: &dyn OAuthHttpClient) -> Result<EmaAccessToken, EmaError> {
        self.exchange_with_clock(http, unix_time).await
    }

    async fn exchange_with_clock(
        self,
        http: &dyn OAuthHttpClient,
        now: impl Fn() -> Result<u64, EmaError> + Sync,
    ) -> Result<EmaAccessToken, EmaError> {
        if self.expires_at <= now()? {
            return Err(EmaError::InvalidRequest("ID-JAG expired before redemption"));
        }
        // Only the assertion carries authority: repeating resource/scope could undo narrowing.
        let token: ResourceTokenResponse = post_form(
            http,
            &self.resource_as,
            &[
                ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
                ("assertion", self.assertion.secret()),
            ],
            EmaExchangeStage::ResourceAuthorizationServer,
            Some(self.expires_at),
            now,
        )
        .await?;
        token.validate(&self.resource, self.scopes)
    }
}

/// A resource-bound bearer with secret-safe diagnostics and its optional lifetime.
#[derive(Clone)]
#[non_exhaustive]
pub struct EmaAccessToken {
    pub access_token: AccessToken,
    pub expires_in: Option<Duration>,
    /// Resource-AS scopes, or the ID-JAG scopes when the response omits `scope`.
    /// Empty when both the ID-JAG and response omit scopes.
    pub scopes: HashSet<String>,
}

impl std::fmt::Debug for EmaAccessToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EmaAccessToken { .. }")
    }
}

/// The endpoint that failed, allowing the caller to apply its own credential lifecycle policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum EmaExchangeStage {
    IdentityProvider,
    ResourceAuthorizationServer,
}

/// Sanitized failures. Raw HTTP adapter errors and provider response bodies are never retained.
#[derive(Debug, Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum EmaError {
    #[error("invalid EMA exchange request: {0}")]
    InvalidRequest(&'static str),
    #[error("invalid EMA response from {stage:?}: {message}")]
    InvalidResponse {
        stage: EmaExchangeStage,
        message: &'static str,
    },
    #[error("EMA request to {0:?} failed")]
    RequestFailed(EmaExchangeStage),
    #[error("{0:?}: invalid_grant")]
    InvalidGrant(EmaExchangeStage),
    #[error("{0:?}: insufficient_user_authentication")]
    InsufficientUserAuthentication(EmaExchangeStage),
    #[error("{stage:?} returned HTTP {status}: {code}")]
    OAuthRejected {
        stage: EmaExchangeStage,
        status: u16,
        code: &'static str,
    },
}

async fn post_form<T: DeserializeOwned>(
    http: &dyn OAuthHttpClient,
    server: &EmaAuthorizationServer,
    params: &[(&str, &str)],
    stage: EmaExchangeStage,
    grant_expires_at: Option<u64>,
    now: impl Fn() -> Result<u64, EmaError> + Sync,
) -> Result<T, EmaError> {
    let response = tokio::time::timeout(DEFAULT_HTTP_TIMEOUT, async {
        // Generate assertions at the request boundary, not when server configuration is built.
        let client_assertion = match &server.client_authentication {
            EmaClientAuthentication::JwtAssertion(provider) => {
                let assertion = provider
                    .create_assertion(server)
                    .await
                    .map_err(|_| EmaError::RequestFailed(stage))?;
                if assertion.0.trim().is_empty() {
                    return Err(EmaError::InvalidRequest(
                        "client assertion must not be empty",
                    ));
                }
                Some(assertion)
            }
            _ => None,
        };
        let request = {
            let mut form = url::form_urlencoded::Serializer::new(String::new());
            form.extend_pairs(params.iter().copied());
            let mut request = oauth2::http::Request::builder()
                .method("POST")
                .uri(&server.token_endpoint)
                .header("content-type", "application/x-www-form-urlencoded")
                .header("accept", "application/json");
            match &server.client_authentication {
                EmaClientAuthentication::ClientSecretBasic(secret) => {
                    let client_id: String =
                        url::form_urlencoded::byte_serialize(server.client_id.as_bytes()).collect();
                    let secret: String =
                        url::form_urlencoded::byte_serialize(secret.secret().as_bytes()).collect();
                    let encoded = STANDARD.encode(format!("{client_id}:{secret}"));
                    let mut header =
                        oauth2::http::HeaderValue::from_str(&format!("Basic {encoded}")).map_err(
                            |_| EmaError::InvalidRequest("invalid client authentication header"),
                        )?;
                    header.set_sensitive(true);
                    request = request.header(oauth2::http::header::AUTHORIZATION, header);
                }
                EmaClientAuthentication::ClientSecretPost(secret) => {
                    form.append_pair("client_id", &server.client_id)
                        .append_pair("client_secret", secret.secret());
                }
                EmaClientAuthentication::None | EmaClientAuthentication::JwtAssertion(_) => {
                    form.append_pair("client_id", &server.client_id);
                }
            }
            if let Some(assertion) = client_assertion {
                form.append_pair(
                    "client_assertion_type",
                    "urn:ietf:params:oauth:client-assertion-type:jwt-bearer",
                )
                .append_pair("client_assertion", &assertion.0);
            }
            request
                .body(form.finish().into_bytes())
                .map_err(|_| EmaError::InvalidRequest("invalid token endpoint URI"))?
        };
        // An external signer may outlive the grant even when it meets the request deadline.
        if let Some(expires_at) = grant_expires_at
            && expires_at <= now()?
        {
            return Err(EmaError::InvalidRequest("ID-JAG expired before redemption"));
        }
        http.execute(OAuthHttpRequest::new(
            request,
            OAuthHttpRedirectPolicy::Stop,
        ))
        .await
        .map_err(|_| EmaError::RequestFailed(stage))
    })
    .await
    .map_err(|_| EmaError::RequestFailed(stage))??;
    let invalid = |message| EmaError::InvalidResponse { stage, message };
    if response.body().len() > MAX_OAUTH_HTTP_RESPONSE_BODY_BYTES {
        return Err(invalid("response body too large"));
    }
    if !response.status().is_success() {
        #[derive(Deserialize)]
        struct OAuthError {
            error: Option<String>,
        }
        let error = serde_json::from_slice::<OAuthError>(response.body()).ok();
        let code = match error.as_ref().and_then(|e| e.error.as_deref()) {
            Some("invalid_grant") => return Err(EmaError::InvalidGrant(stage)),
            Some("insufficient_user_authentication") => {
                return Err(EmaError::InsufficientUserAuthentication(stage));
            }
            Some("invalid_request") => "invalid_request",
            Some("invalid_client") => "invalid_client",
            Some("invalid_scope") => "invalid_scope",
            Some("invalid_target") => "invalid_target",
            Some("unauthorized_client") => "unauthorized_client",
            Some("unsupported_grant_type") => "unsupported_grant_type",
            Some("access_denied") => "access_denied",
            Some("temporarily_unavailable") => "temporarily_unavailable",
            Some("server_error") => "server_error",
            _ => "OAuth token request rejected",
        };
        return Err(EmaError::OAuthRejected {
            stage,
            status: response.status().as_u16(),
            code,
        });
    }
    serde_json::from_slice(response.body()).map_err(|_| invalid("malformed token response"))
}

fn validate_endpoint(value: &str) -> Result<(), EmaError> {
    let url = Url::parse(value).map_err(|_| EmaError::InvalidRequest("invalid endpoint URL"))?;
    let loopback = match url.host() {
        Some(Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        Some(Host::Ipv4(ip)) => ip.is_loopback(),
        Some(Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    };
    if (url.scheme() != "https" && !(url.scheme() == "http" && loopback))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(EmaError::InvalidRequest(
            "endpoint must use HTTPS or HTTP loopback without userinfo or fragments",
        ));
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Resource {
    Single(String),
    Multiple(Vec<String>),
}

impl Resource {
    fn is_exact(&self, expected: &str) -> bool {
        match self {
            Self::Single(value) => value == expected,
            Self::Multiple(values) => values.as_slice() == [expected],
        }
    }
}

#[derive(Deserialize)]
struct JwtHeader {
    alg: String,
    typ: Option<String>,
}

#[derive(Deserialize)]
struct IdJagClaims {
    iss: String,
    sub: String,
    aud: Resource,
    client_id: String,
    jti: String,
    exp: u64,
    iat: u64,
    resource: Resource,
    scope: Option<String>,
    authorization_details: Option<Vec<serde_json::Value>>,
}

#[derive(Deserialize)]
struct IdJagResponse {
    access_token: String,
    issued_token_type: String,
    token_type: String,
    resource: Option<Resource>,
    scope: Option<String>,
    refresh_token: Option<String>,
    authorization_details: Option<Vec<serde_json::Value>>,
}

impl IdJagResponse {
    fn validate(
        &self,
        request: &EmaExchangeRequest<'_>,
        requested: &HashSet<&str>,
    ) -> Result<(HashSet<String>, u64), EmaError> {
        let invalid = |message| EmaError::InvalidResponse {
            stage: EmaExchangeStage::IdentityProvider,
            message,
        };
        if self.issued_token_type != ID_JAG_TOKEN_TYPE
            || self.token_type != "N_A"
            || self.refresh_token.is_some()
        {
            return Err(invalid("unsupported ID-JAG token type or refresh token"));
        }
        let mut parts = self.access_token.split('.');
        let (Some(header), Some(payload), Some(signature), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(invalid("ID-JAG must be a compact signed JWT"));
        };
        if header.is_empty() || payload.is_empty() || signature.is_empty() {
            return Err(invalid("ID-JAG contains an empty JWT segment"));
        }
        let decode = |value| {
            URL_SAFE_NO_PAD
                .decode(value)
                .map_err(|_| invalid("malformed ID-JAG encoding"))
        };
        decode(signature)?;
        let header: JwtHeader = serde_json::from_slice(&decode(header)?)
            .map_err(|_| invalid("malformed ID-JAG header"))?;
        let claims: IdJagClaims = serde_json::from_slice(&decode(payload)?)
            .map_err(|_| invalid("malformed ID-JAG claims"))?;
        if [&self.authorization_details, &claims.authorization_details]
            .into_iter()
            .any(|details| details.as_ref().is_some_and(|details| !details.is_empty()))
        {
            return Err(invalid("authorization_details is not supported"));
        }
        if header.alg.trim().is_empty()
            || header.alg.eq_ignore_ascii_case("none")
            || header.typ.as_deref() != Some("oauth-id-jag+jwt")
            || claims.iss != request.idp.issuer
            || !claims.aud.is_exact(&request.resource_as.issuer)
            || claims.client_id != request.resource_as.client_id
            || claims.sub.trim().is_empty()
            || claims.jti.trim().is_empty()
        {
            return Err(invalid(
                "ID-JAG type, issuer, audience, client, subject, or JWT ID mismatch",
            ));
        }
        let now = unix_time()?;
        if claims.exp <= now || claims.iat > now.saturating_add(60) {
            return Err(invalid("expired or future-issued ID-JAG"));
        }
        if !claims.resource.is_exact(request.resource)
            || self
                .resource
                .as_ref()
                .is_some_and(|r| !r.is_exact(request.resource))
        {
            return Err(invalid("ID-JAG resource mismatch"));
        }
        let parse =
            |scope| parse_scope(scope).ok_or_else(|| invalid("malformed or duplicate scopes"));
        let granted = match claims.scope.as_deref() {
            Some(scope) => parse(scope)?,
            None if requested.is_empty() => HashSet::new(),
            None => return Err(invalid("ID-JAG is missing requested scope authorization")),
        };
        if !requested.is_empty() && !granted.is_subset(requested) {
            return Err(invalid("ID-JAG scope exceeds the request"));
        }
        match self.scope.as_deref() {
            Some(scope) if parse(scope)? != granted => {
                return Err(invalid("response scope differs from ID-JAG scope"));
            }
            None if !requested.is_empty() && granted != *requested => {
                return Err(invalid("response omitted narrowed scope"));
            }
            _ => {}
        }
        Ok((granted.into_iter().map(str::to_owned).collect(), claims.exp))
    }
}

fn parse_scope(scope: &str) -> Option<HashSet<&str>> {
    let scopes: HashSet<_> = scope.split(' ').collect();
    (scopes.iter().all(|s| is_scope_token(s)) && scopes.len() == scope.split(' ').count())
        .then_some(scopes)
}

fn is_scope_token(scope: &str) -> bool {
    !scope.is_empty()
        && scope
            .bytes()
            .all(|b| matches!(b, b'!' | b'#'..=b'[' | b']'..=b'~'))
}

fn unix_time() -> Result<u64, EmaError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| EmaError::InvalidRequest("system clock precedes the Unix epoch"))
}

#[derive(Deserialize)]
struct ResourceTokenResponse {
    access_token: String,
    token_type: String,
    expires_in: Option<u64>,
    resource: Option<Resource>,
    scope: Option<String>,
    refresh_token: Option<String>,
    authorization_details: Option<Vec<serde_json::Value>>,
}

impl ResourceTokenResponse {
    fn validate(
        self,
        resource: &str,
        granted: HashSet<String>,
    ) -> Result<EmaAccessToken, EmaError> {
        let invalid = |message| EmaError::InvalidResponse {
            stage: EmaExchangeStage::ResourceAuthorizationServer,
            message,
        };
        if self
            .authorization_details
            .as_ref()
            .is_some_and(|details| !details.is_empty())
        {
            return Err(invalid("authorization_details is not supported"));
        }
        if !self.token_type.eq_ignore_ascii_case("bearer")
            || self.access_token.trim().is_empty()
            || self.refresh_token.is_some()
            || self.expires_in == Some(0)
        {
            return Err(invalid(
                "invalid bearer token, lifetime, or unexpected refresh token",
            ));
        }
        // The resource need not be echoed, but must agree with the ID-JAG if present.
        if self
            .resource
            .as_ref()
            .is_some_and(|r| !r.is_exact(resource))
        {
            return Err(invalid("access token resource mismatch"));
        }
        // An omitted scope retains the authority carried by the assertion.
        let scopes = if let Some(scope) = self.scope.as_deref() {
            let scopes =
                parse_scope(scope).ok_or_else(|| invalid("malformed or duplicate scopes"))?;
            if !scopes.iter().all(|s| granted.contains(*s)) {
                return Err(invalid("access token scope exceeds ID-JAG scope"));
            }
            scopes.into_iter().map(str::to_owned).collect()
        } else {
            granted
        };
        Ok(EmaAccessToken {
            access_token: AccessToken::new(self.access_token),
            expires_in: self.expires_in.map(Duration::from_secs),
            scopes,
        })
    }
}

#[cfg(test)]
#[path = "enterprise_tests.rs"]
mod tests;
