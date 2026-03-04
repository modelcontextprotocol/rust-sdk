use std::{convert::Infallible, net::SocketAddr};

use axum::{
    Router,
    body::Body,
    http::{Request, Response, StatusCode},
    routing::{get, post},
};
use rmcp::transport::auth::{ClientCredentialsConfig, OAuthState};

fn json_response(body: serde_json::Value) -> Response<Body> {
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&body).unwrap()))
        .unwrap()
}

fn json_error(status: StatusCode, body: serde_json::Value) -> Response<Body> {
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&body).unwrap()))
        .unwrap()
}

async fn resource_metadata_handler(req: Request<Body>) -> Result<Response<Body>, Infallible> {
    let host = req.headers().get("host").unwrap().to_str().unwrap();
    let base_url = format!("http://{}", host);
    Ok(json_response(serde_json::json!({
        "resource": base_url,
        "authorization_servers": [base_url],
        "scopes_supported": ["read", "write"]
    })))
}

async fn auth_server_metadata_handler(req: Request<Body>) -> Result<Response<Body>, Infallible> {
    let host = req.headers().get("host").unwrap().to_str().unwrap();
    let base_url = format!("http://{}", host);
    Ok(json_response(serde_json::json!({
        "issuer": base_url,
        "authorization_endpoint": format!("{}/authorize", base_url),
        "token_endpoint": format!("{}/token", base_url),
        "token_endpoint_auth_methods_supported": ["client_secret_post", "client_secret_basic"],
        "grant_types_supported": ["client_credentials"],
        "scopes_supported": ["read", "write"]
    })))
}

async fn token_handler(req: Request<Body>) -> Result<Response<Body>, Infallible> {
    let body_bytes = axum::body::to_bytes(req.into_body(), 1024 * 64)
        .await
        .unwrap();
    let body_str = String::from_utf8(body_bytes.to_vec()).unwrap();

    // Parse form-urlencoded body
    let params: Vec<(String, String)> = url::form_urlencoded::parse(body_str.as_bytes())
        .into_owned()
        .collect();

    let get_param = |key: &str| -> Option<String> {
        params
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
    };

    let grant_type = get_param("grant_type").unwrap_or_default();
    if grant_type != "client_credentials" {
        return Ok(json_error(
            StatusCode::BAD_REQUEST,
            serde_json::json!({
                "error": "unsupported_grant_type",
                "error_description": "Only client_credentials grant type is supported"
            }),
        ));
    }

    let client_id = get_param("client_id").unwrap_or_default();
    if client_id != "test-m2m-client" {
        return Ok(json_error(
            StatusCode::UNAUTHORIZED,
            serde_json::json!({
                "error": "invalid_client",
                "error_description": "Unknown client_id"
            }),
        ));
    }

    let client_secret = get_param("client_secret").unwrap_or_default();
    if client_secret != "test-m2m-secret" {
        return Ok(json_error(
            StatusCode::UNAUTHORIZED,
            serde_json::json!({
                "error": "invalid_client",
                "error_description": "Invalid client_secret"
            }),
        ));
    }

    let scope = get_param("scope").unwrap_or_default();

    let mut response = serde_json::json!({
        "access_token": "m2m-access-token-12345",
        "token_type": "Bearer",
        "expires_in": 3600
    });

    if !scope.is_empty() {
        response["scope"] = serde_json::Value::String(scope);
    }

    Ok(json_response(response))
}

async fn start_mock_server() -> (String, SocketAddr) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base_url = format!("http://{}", addr);

    let app = Router::new()
        .route(
            "/.well-known/oauth-protected-resource",
            get(resource_metadata_handler),
        )
        .route(
            "/.well-known/oauth-authorization-server",
            get(auth_server_metadata_handler),
        )
        .route("/token", post(token_handler));

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    (base_url, addr)
}

#[tokio::test]
async fn test_client_credentials_flow_client_secret() {
    let (base_url, _addr) = start_mock_server().await;

    let mut oauth_state = OAuthState::new(&base_url, None).await.unwrap();

    let config = ClientCredentialsConfig::ClientSecret {
        client_id: "test-m2m-client".to_string(),
        client_secret: "test-m2m-secret".to_string(),
        scopes: vec!["read".to_string(), "write".to_string()],
        resource: Some(base_url.clone()),
    };

    oauth_state
        .authenticate_client_credentials(config)
        .await
        .unwrap();

    let manager = oauth_state
        .into_authorization_manager()
        .expect("Should be in Authorized state");

    let token = manager.get_access_token().await.unwrap();
    assert_eq!(token, "m2m-access-token-12345");
}

#[tokio::test]
async fn test_client_credentials_invalid_secret() {
    let (base_url, _addr) = start_mock_server().await;

    let mut oauth_state = OAuthState::new(&base_url, None).await.unwrap();

    let config = ClientCredentialsConfig::ClientSecret {
        client_id: "test-m2m-client".to_string(),
        client_secret: "wrong-secret".to_string(),
        scopes: vec![],
        resource: Some(base_url.clone()),
    };

    let result = oauth_state.authenticate_client_credentials(config).await;
    assert!(result.is_err(), "Should fail with invalid credentials");
}

#[tokio::test]
async fn test_client_credentials_invalid_client_id() {
    let (base_url, _addr) = start_mock_server().await;

    let mut oauth_state = OAuthState::new(&base_url, None).await.unwrap();

    let config = ClientCredentialsConfig::ClientSecret {
        client_id: "unknown-client".to_string(),
        client_secret: "test-m2m-secret".to_string(),
        scopes: vec![],
        resource: Some(base_url.clone()),
    };

    let result = oauth_state.authenticate_client_credentials(config).await;
    assert!(result.is_err(), "Should fail with unknown client_id");
}
