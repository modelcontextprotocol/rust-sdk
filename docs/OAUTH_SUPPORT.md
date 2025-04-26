# Model Context Protocol OAuth Authorization

This document describes the OAuth 2.1 authorization implementation for Model Context Protocol (MCP), following the [MCP 2025-03-26 Authorization Specification](https://spec.modelcontextprotocol.io/specification/2025-03-26/basic/authorization/).

## Features

- Full support for OAuth 2.1 authorization flow
- PKCE support for enhanced security
- Authorization server metadata discovery
- Dynamic client registration
- Automatic token refresh
- Authorized SSE transport implementation

## Usage Guide

### 1. Enable Features

Enable the auth feature in Cargo.toml:

```toml
[dependencies]
rmcp = { version = "0.1", features = ["auth", "transport-sse"] }
```

### 2. Create Authorization Manager

```rust ignore
use std::sync::Arc;
use rmcp::transport::auth::AuthorizationManager;

async fn main() -> anyhow::Result<()> {
    // Create authorization manager
    let auth_manager = Arc::new(AuthorizationManager::new("https://api.example.com/mcp").await?);
    
    Ok(())
}
```

### 3. Create Authorization Session and Get Authorization

```rust ignore
use rmcp::transport::auth::AuthorizationSession;

async fn get_authorization(auth_manager: Arc<AuthorizationManager>) -> anyhow::Result<()> {
    // Create authorization session
    let session = AuthorizationSession::new(
        auth_manager.clone(),
        &["mcp"], // Requested scopes
        "http://localhost:8080/callback", // Redirect URI
    ).await?;
    
    // Get authorization URL and guide user to open it
    let auth_url = session.get_authorization_url();
    println!("Please open the following URL in your browser for authorization:\n{}", auth_url);
    
    // Handle callback - In real applications, this is typically done in a callback server
    let auth_code = "Authorization code obtained from browser after user authorization";
    let credentials = session.handle_callback(auth_code).await?;
    
    println!("Authorization successful, access token: {}", credentials.access_token);
    
    Ok(())
}
```

### 4. Use Authorized SSE Transport

```rust
use rmcp::{ServiceExt, model::ClientInfo, transport::create_authorized_transport};

async fn connect_with_auth(auth_manager: Arc<AuthorizationManager>) -> anyhow::Result<()> {
    // Create authorized SSE transport
    let transport = create_authorized_transport(
        "https://api.example.com/mcp",
        auth_manager.clone(),
        None
    ).await?;
    
    // Create client
    let client_service = ClientInfo::default();
    let client = client_service.serve(transport).await?;
    
    // Use client to call APIs
    let tools = client.peer().list_all_tools().await?;
    
    for tool in tools {
        println!("Tool: {} - {}", tool.name, tool.description);
    }
    
    Ok(())
}
```

### 5. Use Authorized HTTP Client

```rust
use rmcp::transport::auth::AuthorizedHttpClient;

async fn make_authorized_request(auth_manager: Arc<AuthorizationManager>) -> anyhow::Result<()> {
    // Create authorized HTTP client
    let client = AuthorizedHttpClient::new(auth_manager, None);
    
    // Send authorized request
    let response = client.get("https://api.example.com/resources").await?;
    let resources = response.json::<Vec<Resource>>().await?;
    
    println!("Number of resources: {}", resources.len());
    
    Ok(())
}
```

## Complete Example
client: Please refer to `examples/clients/src/oauth_client.rs` for a complete usage example.
server: Please refer to `examples/servers/src/mcp_oauth_server.rs` for a complete usage example.
### Running the Example in server
```bash
# Run example
cargo run --example mcp_oauth_server
```

### Running the Example in client

```bash
# Run example
cargo run --example oauth-client
```

## Authorization Flow Description

1. **Metadata Discovery**: Client attempts to get authorization server metadata from `/.well-known/oauth-authorization-server`
2. **Client Registration**: If supported, client dynamically registers itself
3. **Authorization Request**: Build authorization URL with PKCE and guide user to access
4. **Authorization Code Exchange**: After user authorization, exchange authorization code for access token
5. **Token Usage**: Use access token for API calls
6. **Token Refresh**: Automatically use refresh token to get new access token when current one expires

## Security Considerations

- All tokens are securely stored in memory
- PKCE implementation prevents authorization code interception attacks
- Automatic token refresh support reduces user intervention
- Only accepts HTTPS connections or secure local callback URIs

## Troubleshooting

If you encounter authorization issues, check the following:

1. Ensure server supports OAuth 2.1 authorization
2. Verify callback URI matches server's allowed redirect URIs
3. Check network connection and firewall settings
4. Verify server supports metadata discovery or dynamic client registration

## References

- [MCP Authorization Specification](https://spec.modelcontextprotocol.io/specification/2025-03-26/basic/authorization/)
- [OAuth 2.1 Specification Draft](https://oauth.net/2.1/)
- [RFC 8414: OAuth 2.0 Authorization Server Metadata](https://datatracker.ietf.org/doc/html/rfc8414)
- [RFC 7591: OAuth 2.0 Dynamic Client Registration Protocol](https://datatracker.ietf.org/doc/html/rfc7591) 