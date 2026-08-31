# Model Context Protocol OAuth Authorization

This document describes the OAuth 2.1 authorization implementation for Model Context Protocol (MCP), following the [MCP Authorization Specification](https://modelcontextprotocol.io/specification/2026-07-28/basic/authorization/).

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
- Injectable OAuth HTTP client for custom network environments
- Opt-in EMA/XAA refresh-token and ID-JAG exchanges for registered public and confidential clients

## Usage Guide

### 1. Enable Features

Enable the auth feature in Cargo.toml:

```toml
[dependencies]
rmcp = { version = "0.1", features = ["auth", "transport-streamable-http-client-reqwest"] }
```

### 2. Configure OAuth network requests

OAuth makes several HTTP requests before the MCP transport is connected:
protected-resource discovery, authorization-server discovery, dynamic client
registration, authorization-code exchange, token refresh, and client credentials
exchange. When no OAuth HTTP client is provided, the SDK sends those requests
with an internally-created `reqwest::Client`.

If you only need to customize reqwest behavior, pass a configured
`reqwest::Client` to `OAuthState::new`. This preserves the caller-provided
reqwest configuration across OAuth operations, including token requests.

```rust ignore
let default_headers = reqwest::header::HeaderMap::new();
let oauth_http_client = reqwest::Client::builder()
    .timeout(std::time::Duration::from_secs(60))
    .default_headers(default_headers)
    .build()?;

let mut oauth_state = OAuthState::new(&server_url, Some(oauth_http_client))
    .await
    .context("Failed to initialize oauth state machine")?;
```

This is useful for proxy, TLS root, connector, timeout, and default-header
configuration while staying within reqwest. The redirect behavior is the
behavior of the provided reqwest client, so configure that client accordingly.
This OAuth HTTP client is separate from the `reqwest::Client` later passed to
`AuthClient::new`, which is used for the authorized MCP transport after tokens
have been obtained.

If OAuth requests must run outside reqwest, implement `OAuthHttpClient` and use
`OAuthState::new_with_oauth_http_client`. The SDK passes each OAuth request to
your implementation with the raw HTTP request, a suggested timeout, and an
`OAuthHttpRedirectPolicy`. `OAuthHttpClientFuture` returns
`OAuthHttpClientError`, so implementations can propagate their native error
types with `?` without flattening their source chains into strings.

```rust ignore
use std::sync::Arc;

use rmcp::transport::{
    OAuthHttpClient, OAuthHttpClientFuture, OAuthHttpRedirectPolicy,
    OAuthHttpRequest, OAuthState,
};

struct MyOAuthHttpClient;

impl OAuthHttpClient for MyOAuthHttpClient {
    fn execute(&self, request: OAuthHttpRequest) -> OAuthHttpClientFuture<'_> {
        Box::pin(async move {
            match request.redirect_policy {
                OAuthHttpRedirectPolicy::Follow => {
                    // Follow redirects according to your HTTP environment.
                }
                OAuthHttpRedirectPolicy::Stop => {
                    // Return redirect responses without following them.
                }
                _ => {
                    // Future redirect policies may be added.
                }
            }

            // Convert `request.request` into your HTTP stack's request type,
            // execute it, then convert the response back into the expected
            // OAuth HTTP response type.
            let response = todo!("send OAuth request");
            Ok(response)
        })
    }
}

let mut oauth_state = OAuthState::new_with_oauth_http_client(
    &server_url,
    Arc::new(MyOAuthHttpClient),
)
.await?;
```

Use this path when OAuth traffic must go through a browser fetch API, a remote
execution environment, a company gateway, a test fake, or any other non-reqwest
transport.

#### Inspect discovery provenance directly

Most applications can use `OAuthState` without calling metadata discovery
directly. When using `AuthorizationManager`, `resolve_metadata()` returns both
the metadata and how it was obtained. A client that supports the 2025-03-26
default-endpoint fallback can continue with synthesized metadata, while a
client that requires server-published metadata should reject that result:

```rust ignore
use rmcp::transport::auth::{AuthorizationManager, AuthorizationMetadataSource};

async fn configure_metadata(
    manager: &mut AuthorizationManager,
    allow_legacy_endpoint_fallback: bool,
) -> anyhow::Result<()> {
    let resolution = manager.resolve_metadata().await?;

    if resolution.source == AuthorizationMetadataSource::LegacyEndpointFallback {
        if !allow_legacy_endpoint_fallback {
            anyhow::bail!("the server did not publish OAuth metadata");
        }

        tracing::warn!(
            "the server did not publish OAuth metadata; using the 2025-03-26 fallback endpoints"
        );
    }

    manager.set_metadata(resolution.metadata);
    Ok(())
}
```

`ProtectedResourceMetadata` and `AuthorizationServerMetadata` indicate
server-published metadata, so clients can proceed with the returned metadata.
`LegacyEndpointFallback` indicates endpoints synthesized for compatibility
with the 2025-03-26 MCP specification. Clients should proceed only when they
intentionally support that legacy behavior; clients using discovery as an
OAuth capability check should treat it as unsupported.

Applications using `OAuthState` do not need to handle these sources directly:
the state machine resolves metadata internally and retains the legacy fallback.
Low-level `AuthorizationManager` users can use
`AuthorizationMetadataSource::is_discovered()` when they only need to
distinguish server-published metadata from synthesized metadata.

### 3. Start authorization with OAuthState

The `OAuthState` state machine manages the full authorization lifecycle.
`start_authorization` accepts an `AuthorizationRequest` describing the client
identity material you have available, and selects a client registration
mechanism following the [spec's priority order](https://modelcontextprotocol.io/specification/2026-07-28/basic/authorization/client-registration):

1. **Pre-registered client information** (`with_preregistered_client`), when
   the client already holds a `client_id` issued out of band
2. **Client ID Metadata Documents** (SEP-991, `with_client_metadata_url`), when
   the authorization server advertises `client_id_metadata_document_supported`
3. **Dynamic Client Registration**, as a fallback when the authorization server
   advertises a `registration_endpoint`

When no scopes are provided, the SDK automatically selects scopes from the
server's WWW-Authenticate header, Protected Resource Metadata, or AS metadata.

```rust ignore
use rmcp::transport::auth::AuthorizationRequest;

// start authorization - pass no scopes to let the SDK auto-select
oauth_state
    .start_authorization(
        AuthorizationRequest::new(MCP_REDIRECT_URI).with_client_name("My MCP Client"),
    )
    .await
    .context("Failed to start authorization")?;
```

If you know the scopes you need, you can still pass them explicitly:

```rust ignore
oauth_state
    .start_authorization(
        AuthorizationRequest::new(MCP_REDIRECT_URI)
            .with_scopes(["mcp", "profile"])
            .with_client_name("My MCP Client"),
    )
    .await
    .context("Failed to start authorization")?;
```

If the client hosts a Client ID Metadata Document (SEP-991), pass its URL; the
SDK uses it when the server supports CIMD and falls back to dynamic
registration otherwise:

```rust ignore
oauth_state
    .start_authorization(
        AuthorizationRequest::new(MCP_REDIRECT_URI)
            .with_client_name("My MCP Client")
            .with_client_metadata_url("https://example.com/client-metadata.json"),
    )
    .await
    .context("Failed to start authorization")?;
```

If the client was registered with the authorization server out of band, provide
the pre-registered credentials; they take priority over every other mechanism:

```rust ignore
oauth_state
    .start_authorization(
        AuthorizationRequest::new(MCP_REDIRECT_URI)
            .with_preregistered_client("my-client-id")
            .with_client_secret("my-client-secret"),
    )
    .await
    .context("Failed to start authorization")?;
```

### 4. Get authorization url and handle callback

```rust ignore
// get authorization URL and guide user to open it
let auth_url = oauth_state.get_authorization_url().await?;
println!("Please open the following URL in your browser for authorization:\n{}", auth_url);

// handle callback - in real applications, this is typically done in a callback server
let auth_code = "Authorization code (`code` param) obtained from browser after user authorization";
let csrf_token = "CSRF token (`state` param) obtained from browser after user authorization";
oauth_state.handle_callback(auth_code, csrf_token).await?;
```

### 5. Use Authorized Streamable HTTP Transport and create client

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

If initialization reports that authorization is required, return to the
application's authorization flow:

```rust ignore
let client = match client_service.serve(transport).await {
    Ok(client) => client,
    Err(error) if error.is_authorization_required() => {
        // Prompt the user and start the application's authorization flow again.
        return Err(error.into());
    }
    Err(error) => return Err(error.into()),
};
```

The predicate covers both missing or expired local OAuth authorization and an
HTTP 401 challenge from the MCP server. Other failures, including transient
token-refresh errors and insufficient scope, return `false`. The original error
is preserved for logging or more detailed handling; the SDK does not start an
authorization flow automatically.

### 6. Handle scope upgrades

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

## Enterprise-managed authorization (EMA/XAA)

The example requires the `rmcp` features `auth-enterprise-managed`, `client`,
`reqwest` (TLS), and `transport-streamable-http-client-reqwest`, plus `oauth2`
version 5. Call the async function from a Tokio runtime.

The exchange profile has these requirements and limits:

- Each authorization server has its own approved client registration and explicit
  `EmaClientAuthentication`: `None`, `ClientSecretBasic`, `ClientSecretPost`, or
  `JwtAssertion`. The SDK does not select methods from metadata or fall back to a
  different method after a failure.
- Input is an enterprise IdP refresh token. The requested MCP resource must match
  the ID-JAG's sole `resource` claim; scope may be omitted or narrowed.
- RAR (`authorization_details`) and DPoP are not supported. Nonempty authorization
  details are rejected at both exchange stages.
- Redemption consumes the SDK's ID-JAG handle and does not retry automatically.
  This is an SDK safety choice, not a protocol requirement that ID-JAGs be single-use.

The [ID-JAG draft recommends confidential clients](https://datatracker.ietf.org/doc/html/draft-ietf-oauth-identity-assertion-authz-grant-04#section-9.1).
The example below uses confidential clients with `client_secret_basic` at both
servers. Use `None` only where that server permits a public client registration.
Discovery, server approval, SSO, credential storage, and reauthentication remain
the application's responsibility. Client-side ID-JAG checks validate structure and
bindings, not signatures; the resource authorization server verifies signatures.

```rust no_run
use oauth2::{ClientSecret, RefreshToken};
use rmcp::{
    ServiceExt,
    model::ClientInfo,
    transport::{
        StreamableHttpClientTransport,
        auth::{
            default_oauth_http_client,
            enterprise::{EmaAuthorizationServer, EmaClientAuthentication, EmaExchangeRequest},
        },
        streamable_http_client::StreamableHttpClientTransportConfig,
    },
};

async fn connect(
    refresh: &RefreshToken,
    idp_client_secret: ClientSecret,
    resource_client_secret: ClientSecret,
) -> Result<(), Box<dyn std::error::Error>> {
    let resource = "https://mcp.example/mcp";
    let http = default_oauth_http_client()?;
    let idp = EmaAuthorizationServer::new(
        "https://idp.example", "https://idp.example/token", "idp-client",
    )
    .with_client_authentication(EmaClientAuthentication::ClientSecretBasic(idp_client_secret));
    let resource_as = EmaAuthorizationServer::new(
        "https://as.example", "https://as.example/token", "mcp-client",
    )
    .with_client_authentication(EmaClientAuthentication::ClientSecretBasic(resource_client_secret));
    let token = EmaExchangeRequest::new(idp, resource_as, resource, refresh)
        .with_scopes(["files.read"])
        .exchange(&http, &http)
        .await?;
    let transport = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(resource)
            .auth_header(token.access_token.secret()),
    );
    let client = ClientInfo::default().serve(transport).await?;
    client.list_tools(Default::default()).await?;
    client.cancel().await?;
    Ok(())
}
```

`auth_header` takes the token without a `Bearer ` prefix. Use it only with the
approved resource and never log it. This transport uses a fixed token; obtain a
new token and reconnect when it expires or is rejected.

For a registration using JWT client authentication, implement
`EmaClientAssertionProvider` with your application's signer and configure
`JwtAssertion` on that server. The provider example below also uses `async-trait`
version 0.1; `AppSigner` represents your application's existing signing service.

```rust ignore
use std::sync::Arc;
use rmcp::transport::auth::enterprise::{
    EmaAuthorizationServer, EmaClientAssertion, EmaClientAssertionProvider,
    EmaClientAuthentication,
};

struct AppAssertionProvider {
    signer: AppSigner,
}

#[async_trait::async_trait]
impl EmaClientAssertionProvider for AppAssertionProvider {
    async fn create_assertion(
        &self,
        server: &EmaAuthorizationServer,
    ) -> Result<EmaClientAssertion, Box<dyn std::error::Error + Send + Sync>> {
        // Sign a new assertion for this registration and approved server.
        let jwt = self.signer.sign_client_assertion(
            &server.client_id, &server.issuer, &server.token_endpoint,
        ).await?;
        Ok(EmaClientAssertion::new(jwt))
    }
}

let resource_as = EmaAuthorizationServer::new(
    "https://as.example", "https://as.example/token", "mcp-client",
)
.with_client_authentication(EmaClientAuthentication::JwtAssertion(Arc::new(
    AppAssertionProvider { signer },
)));
```

The SDK calls the provider before each token request, including delayed
`EmaIdJag::exchange` redemption. Sign a fresh, short-lived assertion with a unique
`jti`, the registered client ID in `iss` and `sub`, and the server's approved
audience in `aud`. Your signer owns the keys and algorithm; the client assertion
is separate from the ID-JAG grant. Signing and HTTP share a 30-second deadline.

The factory honors per-request redirect policy with the SDK's default reqwest
settings. For custom proxy, CA, or remote-execution policy, implement
`OAuthHttpClient`; use separate adapters for the IdP and resource AS when their
network policies differ.

## Complete Examples

- **Authorization Code client**: [`examples/clients/src/auth/oauth_client.rs`](../examples/clients/src/auth/oauth_client.rs)
- **Client Credentials client**: [`examples/clients/src/auth/client_credentials.rs`](../examples/clients/src/auth/client_credentials.rs)
- **Server**: [`examples/servers/src/complex_auth_streamhttp.rs`](../examples/servers/src/complex_auth_streamhttp.rs)

### Running the Examples

```bash
# Run the OAuth server
cargo run -p mcp-server-examples --example servers_complex_auth_streamhttp

# Run the OAuth client (in another terminal)
cargo run -p mcp-client-examples --example clients_oauth_client

# Run the Client Credentials client
cargo run -p mcp-client-examples --example clients_client_credentials -- \
  <server_url> <client_id> <client_secret>
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
9. **Token Refresh**: Automatically use refresh token to get new access token when current one expires; previously granted scopes are forwarded in the refresh request so providers that require them (e.g. Azure AD v2) work correctly
10. **Scope Upgrade**: On 403 insufficient_scope, compute scope union and re-authorize with upgraded scopes

## Security Considerations

- **PKCE S256 always enforced**: never falls back to `plain` or no challenge. OAuth 2.1 mandates S256 as Mandatory To Implement for servers.
- **RFC 8707 resource binding**: authorization and token requests include the `resource` parameter to bind tokens to the protected resource
- **Redirect policy is explicit for custom OAuth clients**: discovery and registration requests use `OAuthHttpRedirectPolicy::Follow`, while token requests use `OAuthHttpRedirectPolicy::Stop` so custom implementations can avoid forwarding credentials to redirected endpoints
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
6. If OAuth requests need custom proxy, TLS, or connector settings, pass a configured reqwest client to `OAuthState::new`
7. If OAuth requests must run through a non-reqwest environment, implement `OAuthHttpClient` and use `OAuthState::new_with_oauth_http_client`
8. Check `tracing` logs at debug level for detailed discovery and validation info

## References

- [MCP Authorization Specification](https://modelcontextprotocol.io/specification/2026-07-28/basic/authorization/)
- [OAuth 2.1 Specification Draft](https://oauth.net/2.1/)
- [RFC 8414: OAuth 2.0 Authorization Server Metadata](https://datatracker.ietf.org/doc/html/rfc8414)
- [RFC 7591: OAuth 2.0 Dynamic Client Registration Protocol](https://datatracker.ietf.org/doc/html/rfc7591)
- [RFC 8707: Resource Indicators for OAuth 2.0](https://datatracker.ietf.org/doc/html/rfc8707)
- [RFC 9728: OAuth 2.0 Protected Resource Metadata](https://datatracker.ietf.org/doc/html/rfc9728)
- [RFC 7636: Proof Key for Code Exchange (PKCE)](https://datatracker.ietf.org/doc/html/rfc7636)
- [RFC 6749 §6: Refreshing an Access Token](https://www.rfc-editor.org/rfc/rfc6749#section-6)
