use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::Arc,
    time::{Duration, SystemTime},
};

use anyhow::Result;
use axum::{
    Json, Router,
    extract::{Form, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use rand::{Rng, distr::Alphanumeric};
use rmcp::transport::{
    StreamableHttpServerConfig,
    streamable_http_server::{session::local::LocalSessionManager, tower::StreamableHttpService},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::RwLock;
use tower_http::cors::{Any, CorsLayer};
use tracing::{error, info};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use url::Url;

// Import Counter tool for MCP service
mod common;
use common::counter::Counter;

const BIND_ADDRESS: &str = "127.0.0.1:3000";

/// In-memory authorization code record
#[derive(Clone, Debug)]
struct AuthCodeRecord {
    client_id: String,
    redirect_uri: String,
    expires_at: SystemTime,
}

#[derive(Clone)]
struct AppState {
    auth_codes: Arc<RwLock<HashMap<String, AuthCodeRecord>>>,
}

impl AppState {
    fn new() -> Self {
        Self {
            auth_codes: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

fn generate_authorization_code() -> String {
    rand::rng()
        .sample_iter(&Alphanumeric)
        .take(32)
        .map(char::from)
        .collect()
}

fn generate_access_token() -> String {
    rand::rng()
        .sample_iter(&Alphanumeric)
        .take(32)
        .map(char::from)
        .collect()
}

/// Validate that the client_id is a URL that meets CIMD mandatory requirements.
/// Mirrors the JS validateClientIdUrl helper.
fn validate_client_id_url(raw: &str) -> Result<String, String> {
    let url = Url::parse(raw)
        .map_err(|_| "invalid_client_id: client_id must be a valid URL".to_string())?;

    // MUST have https scheme
    if url.scheme() != "https" {
        return Err("invalid_client_id: client_id URL MUST use https scheme".to_string());
    }

    // MUST contain a path component (cannot be empty or just "/")
    let path = url.path();
    if path.is_empty() || path == "/" {
        return Err("invalid_client_id: client_id URL MUST contain a path component".to_string());
    }

    // MUST NOT contain single-dot or double-dot path segments
    if path.split('/').any(|s| s == "." || s == "..") {
        return Err(
            "invalid_client_id: client_id URL MUST NOT contain single-dot or double-dot path segments"
                .to_string(),
        );
    }

    // MUST NOT contain a fragment component
    if url.fragment().is_some() {
        return Err(
            "invalid_client_id: client_id URL MUST NOT contain a fragment component".to_string(),
        );
    }

    // MUST NOT contain a username or password
    if !url.username().is_empty() || url.password().is_some() {
        return Err(
            "invalid_client_id: client_id URL MUST NOT contain a username or password component"
                .to_string(),
        );
    }

    Ok(url.to_string())
}

/// Fetch and validate the client metadata document from the client_id URL.
/// Implements MUST / MUST NOT rules from CIMD section 4.1.
async fn fetch_and_validate_client_metadata(client_id_url: &str) -> Result<Value, String> {
    let client = reqwest::Client::new();
    let res = client
        .get(client_id_url)
        .header(
            reqwest::header::ACCEPT,
            "application/json, application/*+json",
        )
        .send()
        .await
        .map_err(|_| "invalid_client: failed to fetch client metadata document".to_string())?;

    if !res.status().is_success() {
        return Err("invalid_client: failed to fetch client metadata document".to_string());
    }

    let json: Value = res
        .json()
        .await
        .map_err(|_| "invalid_client: client metadata document is not valid JSON".to_string())?;

    if !json.is_object() {
        return Err("invalid_client: client metadata document must be a JSON object".to_string());
    }

    // MUST contain a client_id property equal to the URL of the document
    let client_id_value = json.get("client_id").ok_or_else(|| {
        "invalid_client: client metadata document MUST contain client_id".to_string()
    })?;
    if client_id_value != client_id_url {
        return Err(
            "invalid_client: client_id property in metadata document MUST match the document URL"
                .to_string(),
        );
    }

    // token_endpoint_auth_method MUST NOT be any shared secret based method
    if let Some(method) = json.get("token_endpoint_auth_method") {
        if let Some(method_str) = method.as_str() {
            let forbidden = [
                "client_secret_post",
                "client_secret_basic",
                "client_secret_jwt",
            ];
            if forbidden.contains(&method_str) || method_str.starts_with("client_secret_") {
                return Err("invalid_client: token_endpoint_auth_method MUST NOT be a shared secret based method".to_string());
            }
        }
    }

    // client_secret and client_secret_expires_at MUST NOT be used
    if json.get("client_secret").is_some() {
        return Err(
            "invalid_client: client_secret MUST NOT be present in client metadata".to_string(),
        );
    }
    if json.get("client_secret_expires_at").is_some() {
        return Err(
            "invalid_client: client_secret_expires_at MUST NOT be present in client metadata"
                .to_string(),
        );
    }

    Ok(json)
}

/// Validate redirect_uri against metadata.redirect_uris (exact match).
fn validate_redirect_uri(requested_redirect_uri: &str, metadata: &Value) -> Result<(), String> {
    let redirect_uris = metadata.get("redirect_uris").ok_or_else(|| {
        "invalid_client: client metadata must include redirect_uris array".to_string()
    })?;

    let arr = redirect_uris
        .as_array()
        .ok_or_else(|| "invalid_client: redirect_uris must be an array".to_string())?;

    let requested = requested_redirect_uri.to_string();
    let found = arr.iter().any(|u| u.as_str() == Some(&requested));

    if !found {
        return Err(
            "invalid_request: redirect_uri MUST exactly match one of the registered redirect_uris"
                .to_string(),
        );
    }

    Ok(())
}

/// Minimal Authorization Server Metadata with CIMD support.
async fn oauth_metadata() -> impl IntoResponse {
    let issuer =
        std::env::var("CIMD_ISSUER").unwrap_or_else(|_| format!("http://{}", BIND_ADDRESS));

    let body = serde_json::json!({
        "issuer": issuer,
        "authorization_endpoint": format!("{}/authorize", issuer),
        "token_endpoint": format!("{}/token", issuer),
        "client_id_metadata_document_supported": true,
    });

    Json(body)
}

#[derive(Debug, Deserialize)]
struct AuthorizeQuery {
    client_id: Option<String>,
    redirect_uri: Option<String>,
    response_type: Option<String>,
    state: Option<String>,
    scope: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LoginForm {
    username: Option<String>,
    password: Option<String>,
    // OAuth params come from hidden form fields
    client_id: Option<String>,
    redirect_uri: Option<String>,
    response_type: Option<String>,
    state: Option<String>,
    scope: Option<String>,
}

fn render_login_form(params: &AuthorizeQuery, error: Option<&str>) -> Html<String> {
    let hidden_fields = [
        ("client_id", params.client_id.as_deref().unwrap_or_default()),
        (
            "redirect_uri",
            params.redirect_uri.as_deref().unwrap_or_default(),
        ),
        (
            "response_type",
            params.response_type.as_deref().unwrap_or_default(),
        ),
        ("state", params.state.as_deref().unwrap_or_default()),
        ("scope", params.scope.as_deref().unwrap_or_default()),
    ]
    .iter()
    .map(|(k, v)| format!(r#"<input type="hidden" name="{k}" value="{v}">"#))
    .collect::<Vec<_>>()
    .join("\n      ");

    let error_html = error
        .map(|e| format!(r#"<div class="error">{}</div>"#, e))
        .unwrap_or_default();

    let html = format!(
        r#"<!doctype html>
<html>
  <head>
    <meta charset="utf-8" />
    <title>OAuth Login - CIMD Server</title>
    <style>
      body {{ font-family: sans-serif; max-width: 400px; margin: 50px auto; padding: 20px; }}
      form {{ border: 1px solid #ddd; padding: 20px; border-radius: 8px; }}
      input {{ width: 100%; padding: 8px; margin: 8px 0; box-sizing: border-box; }}
      button {{ background: #007bff; color: white; padding: 10px 20px; border: none; border-radius: 4px; cursor: pointer; width: 100%; }}
      button:hover {{ background: #0056b3; }}
      .error {{ color: red; margin-bottom: 10px; }}
    </style>
  </head>
  <body>
    <h1>OAuth Login</h1>
    {error_html}
    <form method="POST" action="/authorize">
      {hidden_fields}
      <label>Username:</label>
      <input type="text" name="username" required autofocus>
      <label>Password:</label>
      <input type="password" name="password" required>
      <button type="submit">Login</button>
    </form>
    <p style="font-size: 0.9em; color: #666; margin-top: 20px;">
      Demo credentials: <strong>admin</strong> / <strong>admin</strong>
    </p>
  </body>
</html>
"#
    );

    Html(html)
}

async fn authorize_get(Query(params): Query<AuthorizeQuery>) -> impl IntoResponse {
    render_login_form(&params, None)
}

async fn authorize_post(
    State(state): State<AppState>,
    Form(form): Form<LoginForm>,
) -> impl IntoResponse {
    // Convert LoginForm (which includes OAuth params from hidden fields) to AuthorizeQuery
    let params = AuthorizeQuery {
        client_id: form.client_id.clone(),
        redirect_uri: form.redirect_uri.clone(),
        response_type: form.response_type.clone(),
        state: form.state.clone(),
        scope: form.scope.clone(),
    };

    match handle_authorize(&state, &params, &form).await {
        Ok(redirect_response) => redirect_response,
        Err(error_response) => error_response,
    }
}

async fn handle_authorize(
    state: &AppState,
    params: &AuthorizeQuery,
    form: &LoginForm,
) -> Result<Response, Response> {
    let client_id_raw = params
        .client_id
        .as_deref()
        .ok_or_else(|| bad_request("invalid_request: client_id is required"))?;
    let redirect_uri = params
        .redirect_uri
        .as_deref()
        .ok_or_else(|| bad_request("invalid_request: redirect_uri is required"))?;
    let response_type = params
        .response_type
        .as_deref()
        .ok_or_else(|| bad_request("invalid_request: response_type is required"))?;

    if response_type != "code" {
        return Err(bad_request(
            "unsupported_response_type: only response_type=code is supported",
        ));
    }

    let client_id_url = validate_client_id_url(client_id_raw).map_err(|e| bad_request(&e))?;
    let metadata = fetch_and_validate_client_metadata(&client_id_url)
        .await
        .map_err(|e| bad_request(&e))?;
    validate_redirect_uri(redirect_uri, &metadata).map_err(|e| bad_request(&e))?;

    // If this is a login POST, validate credentials
    if let (Some(username), Some(password)) = (&form.username, &form.password) {
        if username != "admin" || password != "admin" {
            let html = render_login_form(params, Some("Invalid username or password"));
            return Err(html.into_response());
        }

        // Login successful - generate authorization code and redirect
        let code = generate_authorization_code();
        let expires_at = SystemTime::now() + Duration::from_secs(10 * 60);

        {
            let mut codes = state.auth_codes.write().await;
            codes.insert(
                code.clone(),
                AuthCodeRecord {
                    client_id: client_id_url,
                    redirect_uri: redirect_uri.to_string(),
                    expires_at,
                },
            );
        }

        let mut url = Url::parse(redirect_uri)
            .map_err(|_| bad_request("invalid_request: redirect_uri is invalid"))?;
        url.query_pairs_mut().append_pair("code", &code);
        if let Some(state_param) = &params.state {
            url.query_pairs_mut().append_pair("state", state_param);
        }

        Ok(Redirect::to(url.as_str()).into_response())
    } else {
        // GET request without credentials: show login form
        let html = render_login_form(params, None);
        Err(html.into_response())
    }
}

fn bad_request(message: &str) -> Response {
    let body = serde_json::json!({
        "error": "invalid_request",
        "error_description": message,
    });
    (StatusCode::BAD_REQUEST, Json(body)).into_response()
}

#[derive(Debug, Deserialize)]
struct TokenRequest {
    grant_type: Option<String>,
    code: Option<String>,
}

async fn token(State(state): State<AppState>, Form(form): Form<TokenRequest>) -> impl IntoResponse {
    if form.grant_type.as_deref() != Some("authorization_code") {
        let body = serde_json::json!({
            "error": "unsupported_grant_type",
            "error_description": "Only authorization_code is supported in this demo",
        });
        return (StatusCode::BAD_REQUEST, Json(body)).into_response();
    }

    let code = match &form.code {
        Some(c) => c.clone(),
        None => {
            let body = serde_json::json!({
                "error": "invalid_request",
                "error_description": "Authorization code is required",
            });
            return (StatusCode::BAD_REQUEST, Json(body)).into_response();
        }
    };

    let record_opt = {
        let mut codes = state.auth_codes.write().await;
        codes.remove(&code)
    };

    let record = match record_opt {
        Some(r) => r,
        None => {
            let body = serde_json::json!({
                "error": "invalid_grant",
                "error_description": "Invalid authorization code",
            });
            return (StatusCode::BAD_REQUEST, Json(body)).into_response();
        }
    };

    if SystemTime::now() > record.expires_at {
        let body = serde_json::json!({
            "error": "invalid_grant",
            "error_description": "Authorization code has expired",
        });
        return (StatusCode::BAD_REQUEST, Json(body)).into_response();
    }

    let access_token = generate_access_token();
    let body = serde_json::json!({
        "access_token": access_token,
        "token_type": "Bearer",
        "expires_in": 3600,
    });

    Json(body).into_response()
}

async fn index() -> Html<&'static str> {
    Html(
        "<html><body><h1>CIMD OAuth + MCP Server</h1><p>This server supports Client ID Metadata Documents (SEP-991) and exposes an MCP endpoint at <code>/mcp</code>.</p></body></html>",
    )
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

    let state = AppState::new();

    // Create streamable HTTP service for MCP
    let mcp_service: StreamableHttpService<Counter, LocalSessionManager> =
        StreamableHttpService::new(
            || Ok(Counter::new()),
            LocalSessionManager::default().into(),
            StreamableHttpServerConfig::default(),
        );

    let addr = BIND_ADDRESS.parse::<SocketAddr>()?;

    let cors_layer = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
        .route("/", get(index))
        .route(
            "/.well-known/oauth-authorization-server",
            get(oauth_metadata),
        )
        .route("/authorize", get(authorize_get).post(authorize_post))
        .route("/token", post(token).layer(cors_layer.clone()))
        .nest_service("/mcp", mcp_service)
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!("CIMD OAuth server listening on http://{}", addr);

    if let Err(e) = axum::serve(listener, app).await {
        error!("server error: {}", e);
    }

    Ok(())
}
