# Model Context Protocol OAuth Authorization

This document describes the OAuth 2.1 authorization implementation for Model Context Protocol (MCP), following the [MCP 2025-11-25 Authorization Specification](https://modelcontextprotocol.io/specification/2025-11-25/basic/authorization/).

## Features

- Full support for OAuth 2.1 authorization flow with PKCE (S256)
- RFC 8707 resource parameter binding
- Protected Resource Metadata discovery (RFC 9728)
- Authorization Server Metadata discovery (RFC 8414 + OpenID Connect)
- Dynamic client registration (RFC 7591)
- Client ID Metadata Documents (CIMD) (SEP-991 / Client ID Metadata Documents )
- Scope selection from WWW-Authenticate, Protected Resource Metadata, and AS metadata
- Scope upgrade on 403 insufficient_scope (SEP-835)
- Automatic token refresh
- Authorized HTTP Client implementation

## Usage Guide

### 1. Enable Features

Enable the auth feature in Cargo.toml:

```toml
[dependencies]
rmcp = { version = "0.1", features = ["auth", "transport-streamable-http-client-reqwest"] }
```

### 2. Use OAuthState

The `OAuthState` state machine manages the full authorization lifecycle. When no
scopes are provided, the SDK automatically selects scopes from the server's
WWW-Authenticate header, Protected Resource Metadata, or AS metadata.

```rust ignore
    // initialize oauth state machine
    let mut oauth_state = OAuthState::new(&server_url, None)
        .await
        .context("Failed to initialize oauth state machine")?;

    // start authorization - pass empty scopes to let the SDK auto-select
    oauth_state
        .start_authorization(&[], MCP_REDIRECT_URI, Some("My MCP Client"))
        .await
        .context("Failed to start authorization")?;
```

If you know the scopes you need, you can still pass them explicitly:

```rust ignore
    oauth_state
        .start_authorization(&["mcp", "profile"], MCP_REDIRECT_URI, Some("My MCP Client"))
        .await
        .context("Failed to start authorization")?;
```

### 3. Get authorization url and handle callback

```rust ignore
    // get authorization URL and guide user to open it
    let auth_url = oauth_state.get_authorization_url().await?;
    println!("Please open the following URL in your browser for authorization:\n{}", auth_url);

    // handle callback - in real applications, this is typically done in a callback server
    let auth_code = "Authorization code (`code` param) obtained from browser after user authorization";
    let csrf_token = "CSRF token (`state` param) obtained from browser after user authorization";
    oauth_state.handle_callback(auth_code, csrf_token).await?;
```

### 4. Use Authorized Streamable HTTP Transport and create client

```rust ignore
    let am = oauth_state
        .into_authorization_manager()
        .ok_or_else(|| anyhow::anyhow!("Failed to get authorization manager"))?;
    let client = AuthClient::new(reqwest::Client::default(), am);
    let transport = StreamableHttpClientTransport::with_client(
        client,
        StreamableHttpClientTransportConfig::with_uri(MCP_SERVER_URL),
    );

    // create client and connect to MCP server
    let client_service = ClientInfo::default();
    let client = client_service.serve(transport).await?;
```

### 5. Handle scope upgrades

If a server returns 403 with `insufficient_scope`, you can request a scope
upgrade. The SDK computes the union of current and required scopes and
transitions back to the session state for re-authorization.

```rust ignore
    match oauth_state.request_scope_upgrade("admin:write", MCP_REDIRECT_URI).await {
        Ok(auth_url) => {
            // open auth_url in browser, handle callback as before
            println!("Re-authorize at: {}", auth_url);
        }
        Err(e) => {
            eprintln!("Scope upgrade failed: {}", e);
        }
    }
```

## Complete Examples

- **Client**: `examples/clients/src/auth/oauth_client.rs`
- **Server**: `examples/servers/src/complex_auth_streamhttp.rs`

### Running the Examples

```bash
# Run the OAuth server
cargo run -p mcp-server-examples --example servers_complex_auth_streamhttp

# Run the OAuth client (in another terminal)
cargo run -p mcp-client-examples --example clients_oauth_client
```

## Authorization Flow Description

1. **Resource Metadata Discovery**: Client probes the server and extracts `WWW-Authenticate` parameters including `resource_metadata` URL and `scope`
2. **Protected Resource Metadata**: Client fetches resource server metadata (RFC 9728) to find authorization server(s) and supported scopes
3. **AS Metadata Discovery**: Client discovers authorization server metadata via RFC 8414 and OpenID Connect well-known endpoints
4. **Client Registration**: If supported, client dynamically registers itself (or uses URL-based Client ID via SEP-991)
5. **Scope Selection**: SDK picks scopes from WWW-Authenticate > PRM > AS metadata > caller defaults
6. **Authorization Request**: Build authorization URL with PKCE (S256) and RFC 8707 resource parameter
7. **Authorization Code Exchange**: After user authorization, exchange code for access token (with resource parameter)
8. **Token Usage**: Use access token for API calls via `AuthClient` or `AuthorizedHttpClient`
9. **Token Refresh**: Automatically use refresh token to get new access token when current one expires
10. **Scope Upgrade**: On 403 insufficient_scope, compute scope union and re-authorize with upgraded scopes

## Security Considerations

- **PKCE S256 always enforced**: never falls back to `plain` or no challenge. OAuth 2.1 mandates S256 as Mandatory To Implement for servers.
- **RFC 8707 resource binding**: authorization and token requests include the `resource` parameter to bind tokens to the protected resource
- All tokens are securely stored in memory (custom credential stores supported)
- Automatic token refresh reduces user intervention
- Server metadata validation warns on non-compliant configurations but proceeds where relatively safe

## Troubleshooting

If you encounter authorization issues, check the following:

1. Ensure server supports OAuth 2.1 authorization
2. Verify callback URI matches server's allowed redirect URIs
3. Check network connection and firewall settings
4. Verify server supports metadata discovery or dynamic client registration
5. If PKCE fails, the server may not support S256 (non-compliant with OAuth 2.1)
6. Check `tracing` logs at debug level for detailed discovery and validation info

## References

- [MCP Authorization Specification (2025-11-25)](https://modelcontextprotocol.io/specification/2025-11-25/basic/authorization/)
- [OAuth 2.1 Specification Draft](https://oauth.net/2.1/)
- [RFC 8414: OAuth 2.0 Authorization Server Metadata](https://datatracker.ietf.org/doc/html/rfc8414)
- [RFC 7591: OAuth 2.0 Dynamic Client Registration Protocol](https://datatracker.ietf.org/doc/html/rfc7591)
- [RFC 8707: Resource Indicators for OAuth 2.0](https://datatracker.ietf.org/doc/html/rfc8707)
- [RFC 9728: OAuth 2.0 Protected Resource Metadata](https://datatracker.ietf.org/doc/html/rfc9728)
- [RFC 7636: Proof Key for Code Exchange (PKCE)](https://datatracker.ietf.org/doc/html/rfc7636)
