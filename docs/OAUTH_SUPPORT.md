# Model Context Protocol OAuth Authorization

This document describes the OAuth 2.1 authorization implementation for Model Context Protocol (MCP), following the [MCP 2025-03-26 Authorization Specification](https://spec.modelcontextprotocol.io/specification/2025-03-26/basic/authorization/).

## Features

- Full support for OAuth 2.1 authorization flow
- PKCE support for enhanced security
- Authorization server metadata discovery
- Dynamic client registration
- Automatic token refresh
- Authorized SSE transport implementation
- Authorized HTTP Client implementation
## Usage Guide

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
    let auth_code = "Authorization code (`code` param) obtained from browser after user authorization";
    let csrf_token = "CSRF token (`state` param) obtained from browser after user authorization";
    let credentials = oauth_state.handle_callback(auth_code, csrf_token).await?;
    
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

### 5. Use Authorized HTTP Client after authorized

```rust ignore
    let client = oauth_state.to_authorized_http_client().await?;
```

## Complete Example
client: Please refer to `examples/clients/src/auth/oauth_client.rs` for a complete usage example.
server: Please refer to `examples/servers/src/complex_auth_sse.rs` for a complete usage example.
### Running the Example in server
```bash
# Run example
cargo run --example complex_auth_sse
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
