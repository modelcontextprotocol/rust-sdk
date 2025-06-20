# Model Context Protocol OAuth Authorization

This document describes the OAuth 2.1 authorization implementation for Model Context Protocol (MCP), following the [MCP 2025-03-26 Authorization Specification](https://modelcontextprotocol.io/specification/2025-03-26/basic/authorization/).

## Features

- Full support for OAuth 2.1 authorization flow
- PKCE support for enhanced security
- Authorization server metadata discovery
- Dynamic client registration
- Automatic token refresh
- Authorized SSE transport implementation
- Authorized HTTP Client implementation
- Server-side OAuth implementation with token validation
- Axum middleware for protecting API endpoints

## Client-Side Implementation

### 1. Enable Features

Enable the auth feature in Cargo.toml:

```toml
[dependencies]
rmcp = { version = "0.1", features = ["auth", "transport-sse-client"] }
```

### 2. Use OAuthState

```rust ignore
    // Initialize oauth state machine
    let mut oauth_state = OAuthState::new(&server_url, None)
        .await
        .context("Failed to initialize oauth state machine")?;
    oauth_state
        .start_authorization(&["mcp", "profile", "email"], MCP_REDIRECT_URI)
        .await
        .context("Failed to start authorization")?;

```

### 3. Get authorization url and do callback

```rust ignore
    // Get authorization URL and guide user to open it
    let auth_url = oauth_state.get_authorization_url().await?;
    println!("Please open the following URL in your browser for authorization:\n{}", auth_url);
    
    // Handle callback - In real applications, this is typically done in a callback server
    let auth_code = "Authorization code obtained from browser after user authorization";
    let credentials = oauth_state.handle_callback(auth_code).await?;
    
    println!("Authorization successful, access token: {}", credentials.access_token);

```

### 4. Use Authorized SSE Transport and create client

```rust ignore
    let transport =
        match create_authorized_transport(MCP_SSE_URL.to_string(), oauth_state, Some(retry_config))
            .await
        {
            Ok(t) => t,
            Err(e) => {
                tracing::error!("Failed to create authorized transport: {}", e);
                return Err(anyhow::anyhow!("Connection failed: {}", e));
            }
        };

    // Create client and connect to MCP server
    let client_service = ClientInfo::default();
    let client = client_service.serve(transport).await?;
```

### 5. May you can use Authorized HTTP Client after authorized

```rust ignore
    let client = oauth_state.to_authorized_http_client().await?;
```

## Server-Side Implementation

The MCP SDK also provides a server-side OAuth implementation for creating OAuth 2.1 compliant MCP servers.

### 1. Enable Server Features

```toml
[dependencies]
rmcp = { version = "0.1", features = ["auth", "axum"] }
```

### 2. Basic Server Setup

```rust
use std::sync::Arc;
use axum::{routing::get, Router};
use rmcp::transport::auth_server::{AuthServer, AuthServerConfig, axum_ext};
use url::Url;

async fn setup_oauth_server() -> anyhow::Result<Router> {
    // Configure server
    let server_base_url = Url::parse("https://api.example.com")?;
    
    // Create OAuth server configuration
    let config = AuthServerConfig {
        client_id: "mcp_server".to_string(),
        client_secret: "mcp_server_secret".to_string(),
        authorize_endpoint: format!("{}/authorize", server_base_url),
        token_endpoint: format!("{}/token", server_base_url),
        registration_endpoint: format!("{}/register", server_base_url),
        issuer: server_base_url.to_string(),
        supported_scopes: vec!["mcp".to_string(), "profile".to_string()],
    };
    
    // Create AuthServer instance
    let auth_server = Arc::new(AuthServer::new(config, server_base_url));
    
    // Create routes
    let oauth_router = Router::new()
        .route("/.well-known/oauth-authorization-server", get(axum_ext::oauth_metadata_handler))
        .route("/register", get(axum_ext::client_registration_handler))
        .with_state(auth_server.clone());
    
    // Create protected API routes
    let protected_api = Router::new()
        .route("/api/resource", get(resource_handler))
        .route_layer(axum::middleware::from_fn_with_state(
            auth_server.clone(),
            axum_ext::auth_middleware,
        ));
    
    // Combine routes
    let app = Router::new()
        .merge(oauth_router)
        .merge(protected_api);
    
    Ok(app)
}
```

### 3. Using Third-Party Authorization Servers

MCP servers can delegate authentication to third-party OAuth servers (identity providers):

```rust
use std::collections::HashMap;
use rmcp::transport::auth_server::{AuthServer, AuthServerConfig, ThirdPartyAuthConfig};

async fn setup_server_with_third_party_auth() -> anyhow::Result<Router> {
    // Local MCP server configuration
    let server_base_url = Url::parse("https://api.example.com")?;
    let config = AuthServerConfig {
        client_id: "mcp_server".to_string(),
        client_secret: "mcp_server_secret".to_string(),
        authorize_endpoint: format!("{}/authorize", server_base_url),
        token_endpoint: format!("{}/token", server_base_url),
        registration_endpoint: format!("{}/register", server_base_url),
        issuer: server_base_url.to_string(),
        supported_scopes: vec!["mcp".to_string(), "profile".to_string()],
    };
    
    // Third-party auth server configuration (e.g., Keycloak, Auth0, etc.)
    let third_party_config = ThirdPartyAuthConfig {
        client_id: "mcp_client_at_idp".to_string(),
        client_secret: "mcp_client_secret_at_idp".to_string(),
        auth_server_url: "https://auth.example.com".to_string(),
        authorize_endpoint: "https://auth.example.com/oauth2/authorize".to_string(),
        token_endpoint: "https://auth.example.com/oauth2/token".to_string(),
        introspection_endpoint: Some("https://auth.example.com/oauth2/introspect".to_string()),
        revocation_endpoint: Some("https://auth.example.com/oauth2/revoke".to_string()),
        supported_scopes: vec!["openid".to_string(), "profile".to_string(), "mcp".to_string()],
        additional_params: HashMap::new(),
    };
    
    // Create AuthServer with third-party delegation
    let auth_server = Arc::new(AuthServer::with_third_party_auth(
        config, 
        third_party_config, 
        server_base_url
    ));
    
    // Create routes as before
    // ...
}
```

When using third-party authentication:
1. The MCP server will forward token validation to the third-party server
2. Token introspection will be performed using the third-party's introspection endpoint
3. Token revocation will be delegated to the third-party's revocation endpoint
4. The MCP server will respect the scope and expiration information from the third-party

### 4. Token Validation

```rust
async fn validate_token(auth_server: &AuthServer, token: &str, required_scope: &str) -> Result<(), ServerAuthError> {
    // Validate token and check scope
    let token_info = auth_server.validate_token_scope(token, required_scope).await?;
    
    // Access token information
    let client_id = &token_info.client_id;
    let username = token_info.username.as_deref().unwrap_or("anonymous");
    
    println!("Token is valid for client {} and user {}", client_id, username);
    Ok(())
}
```

### 5. Server-Side Token Management

The server-side implementation provides methods for token management:

```rust
// Register a test token
let test_token = "test_token_123456".to_string();
let token_info = TokenInfo {
    active: true,
    scope: Some("mcp profile".to_string()),
    client_id: "test_client".to_string(),
    username: Some("test_user".to_string()),
    exp: Some(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs() + 3600, // 1 hour expiration
    ),
    additional_fields: Default::default(),
};
auth_server.register_token(test_token, token_info).await?;

// Revoke a token
auth_server.revoke_token("some_token_to_revoke").await?;
```

## Complete Examples

### Client Example
Please refer to `examples/clients/src/auth/oauth_client.rs` for a complete client usage example.

### Server Examples
- Basic server: `examples/servers/auth_server.rs`
- Integration test: `examples/auth_integration/main.rs`
- Third-party auth: `examples/third_party_auth_server.rs`

### Running the Examples

```bash
# Run the client example
cargo run --example clients_oauth_client

# Run the server example
cargo run --example auth_server

# Run the integration test
cargo run --example auth_integration

# Run the third-party auth server example
cargo run --example third_party_auth_server
```

## Authorization Flow Description

1. **Metadata Discovery**: Client attempts to get authorization server metadata from `/.well-known/oauth-authorization-server`
2. **Client Registration**: If supported, client dynamically registers itself
3. **Authorization Request**: Build authorization URL with PKCE and guide user to access
4. **Authorization Code Exchange**: After user authorization, exchange authorization code for access token
5. **Token Usage**: Use access token for API calls
6. **Token Refresh**: Automatically use refresh token to get new access token when current one expires

## Security Considerations

- Tokens are validated for expiration and appropriate scopes
- Bearer tokens are extracted securely from authorization headers
- Redirect URIs are validated according to MCP security requirements (HTTPS or localhost)
- PKCE implementation prevents authorization code interception attacks
- Token revocation is supported for invalidating access
- Automatic token refresh support reduces user intervention

## Best Practices

1. Always use HTTPS in production
2. Implement proper token expiration
3. Use proper scope validation for each protected endpoint
4. Store client secrets securely
5. Implement rate limiting for registration and token endpoints

## Troubleshooting

If you encounter authorization issues, check the following:

1. Ensure server supports OAuth 2.1 authorization
2. Verify callback URI matches server's allowed redirect URIs
3. Check network connection and firewall settings
4. Verify server supports metadata discovery or dynamic client registration
5. For server-side issues, check that token validation includes proper scopes

## References

- [MCP Authorization Specification](https://modelcontextprotocol.io/specification/2025-03-26/basic/authorization/)
- [OAuth 2.1 Specification Draft](https://oauth.net/2.1/)
- [RFC 8414: OAuth 2.0 Authorization Server Metadata](https://datatracker.ietf.org/doc/html/rfc8414)
- [RFC 7591: OAuth 2.0 Dynamic Client Registration Protocol](https://datatracker.ietf.org/doc/html/rfc7591) 