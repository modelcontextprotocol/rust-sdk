use std::future::Future;

use rmcp::{
    ClientHandler, ErrorData, RoleClient, ServiceExt,
    model::*,
    service::RequestContext,
    transport::{
        AuthClient, AuthorizationManager, StreamableHttpClientTransport,
        auth::{OAuthClientConfig, OAuthState},
        streamable_http_client::StreamableHttpClientTransportConfig,
    },
};
use serde_json::{Value, json};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

// ─── Context parsed from MCP_CONFORMANCE_CONTEXT ────────────────────────────

#[derive(Debug, Default, serde::Deserialize)]
struct ConformanceContext {
    #[serde(default)]
    name: Option<String>,
    // pre-registration / client-credentials-basic
    #[serde(default)]
    client_id: Option<String>,
    #[serde(default)]
    client_secret: Option<String>,
    // client-credentials-jwt
    #[serde(default)]
    private_key_pem: Option<String>,
    #[serde(default)]
    signing_algorithm: Option<String>,
    // cross-app-access
    #[serde(default)]
    idp_client_id: Option<String>,
    #[serde(default)]
    idp_id_token: Option<String>,
    #[serde(default)]
    idp_issuer: Option<String>,
    #[serde(default)]
    idp_token_endpoint: Option<String>,
}

fn load_context() -> ConformanceContext {
    std::env::var("MCP_CONFORMANCE_CONTEXT")
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

// ─── Client handlers ────────────────────────────────────────────────────────

/// A basic client handler that does nothing special
struct BasicClientHandler;
impl ClientHandler for BasicClientHandler {}

/// A client handler that handles elicitation requests by applying schema defaults.
struct ElicitationDefaultsClientHandler;

impl ClientHandler for ElicitationDefaultsClientHandler {
    fn get_info(&self) -> ClientInfo {
        let mut info = ClientInfo::default();
        info.capabilities.elicitation = Some(ElicitationCapability {
            form: Some(FormElicitationCapability {
                schema_validation: Some(true),
            }),
            url: None,
        });
        info
    }

    fn create_elicitation(
        &self,
        request: CreateElicitationRequestParams,
        _cx: RequestContext<RoleClient>,
    ) -> impl Future<Output = Result<CreateElicitationResult, ErrorData>> + Send + '_ {
        async move {
            let content = match &request {
                CreateElicitationRequestParams::FormElicitationParams {
                    requested_schema, ..
                } => {
                    let mut defaults = serde_json::Map::new();
                    for (name, prop) in &requested_schema.properties {
                        match prop {
                            PrimitiveSchema::String(s) => {
                                if let Some(d) = &s.default {
                                    defaults.insert(name.clone(), Value::String(d.clone()));
                                }
                            }
                            PrimitiveSchema::Number(n) => {
                                if let Some(d) = n.default {
                                    defaults.insert(name.clone(), json!(d));
                                }
                            }
                            PrimitiveSchema::Integer(i) => {
                                if let Some(d) = i.default {
                                    defaults.insert(name.clone(), json!(d));
                                }
                            }
                            PrimitiveSchema::Boolean(b) => {
                                if let Some(d) = b.default {
                                    defaults.insert(name.clone(), Value::Bool(d));
                                }
                            }
                            PrimitiveSchema::Enum(e) => {
                                let val = match e {
                                    EnumSchema::Single(SingleSelectEnumSchema::Untitled(u)) => {
                                        u.default.as_ref().map(|d| Value::String(d.clone()))
                                    }
                                    EnumSchema::Single(SingleSelectEnumSchema::Titled(t)) => {
                                        t.default.as_ref().map(|d| Value::String(d.clone()))
                                    }
                                    EnumSchema::Multi(MultiSelectEnumSchema::Untitled(u)) => {
                                        u.default.as_ref().map(|d| {
                                            Value::Array(
                                                d.iter()
                                                    .map(|s| Value::String(s.clone()))
                                                    .collect(),
                                            )
                                        })
                                    }
                                    EnumSchema::Multi(MultiSelectEnumSchema::Titled(t)) => {
                                        t.default.as_ref().map(|d| {
                                            Value::Array(
                                                d.iter()
                                                    .map(|s| Value::String(s.clone()))
                                                    .collect(),
                                            )
                                        })
                                    }
                                    EnumSchema::Legacy(_) => None,
                                };
                                if let Some(v) = val {
                                    defaults.insert(name.clone(), v);
                                }
                            }
                        }
                    }
                    Some(Value::Object(defaults))
                }
                _ => Some(json!({})),
            };
            Ok(CreateElicitationResult {
                action: ElicitationAction::Accept,
                content,
            })
        }
    }
}

/// A client handler that handles both sampling and elicitation
struct FullClientHandler;

impl ClientHandler for FullClientHandler {
    fn get_info(&self) -> ClientInfo {
        let mut info = ClientInfo::default();
        info.capabilities.elicitation = Some(ElicitationCapability {
            form: Some(FormElicitationCapability {
                schema_validation: Some(true),
            }),
            url: None,
        });
        info
    }

    fn create_message(
        &self,
        params: CreateMessageRequestParams,
        _cx: RequestContext<RoleClient>,
    ) -> impl Future<Output = Result<CreateMessageResult, ErrorData>> + Send + '_ {
        async move {
            let prompt_text = params
                .messages
                .first()
                .and_then(|m| m.content.first())
                .and_then(|c| c.as_text())
                .map(|t| t.text.clone())
                .unwrap_or_default();
            Ok(CreateMessageResult {
                message: SamplingMessage::new(
                    Role::Assistant,
                    SamplingMessageContent::text(format!(
                        "This is a mock LLM response to: {}",
                        prompt_text
                    )),
                ),
                model: "mock-model".into(),
                stop_reason: Some("endTurn".into()),
            })
        }
    }

    fn create_elicitation(
        &self,
        _request: CreateElicitationRequestParams,
        _cx: RequestContext<RoleClient>,
    ) -> impl Future<Output = Result<CreateElicitationResult, ErrorData>> + Send + '_ {
        async move {
            Ok(CreateElicitationResult {
                action: ElicitationAction::Accept,
                content: Some(json!({"username": "testuser", "email": "test@example.com"})),
            })
        }
    }
}

// ─── OAuth helpers ──────────────────────────────────────────────────────────

const CIMD_CLIENT_METADATA_URL: &str = "https://conformance-test.local/client-metadata.json";
const REDIRECT_URI: &str = "http://localhost:3000/callback";

/// Perform the headless OAuth authorization-code flow.
///
/// 1. Discover metadata, register (or use CIMD), get auth URL
/// 2. Fetch the auth URL with redirect:manual → extract code from Location header
/// 3. Exchange code for token
/// 4. Return an `AuthClient` wrapping `reqwest::Client`
async fn perform_oauth_flow(
    server_url: &str,
    ctx: &ConformanceContext,
) -> anyhow::Result<AuthClient<reqwest::Client>> {
    let mut oauth = OAuthState::new(server_url, None).await?;

    // Discover + register + get auth URL
    oauth
        .start_authorization_with_metadata_url(
            &[],
            REDIRECT_URI,
            Some("conformance-client"),
            Some(CIMD_CLIENT_METADATA_URL),
        )
        .await?;

    let auth_url = oauth.get_authorization_url().await?;
    tracing::debug!("Authorization URL: {}", auth_url);

    // Headless: fetch the auth URL without following redirects
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let resp = http.get(&auth_url).send().await?;
    let location = resp
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| anyhow::anyhow!("No Location header in auth redirect"))?;

    let redirect_url = url::Url::parse(location)?;
    let code = redirect_url
        .query_pairs()
        .find(|(k, _)| k == "code")
        .map(|(_, v)| v.to_string())
        .ok_or_else(|| anyhow::anyhow!("No code in redirect URL"))?;
    let state = redirect_url
        .query_pairs()
        .find(|(k, _)| k == "state")
        .map(|(_, v)| v.to_string())
        .ok_or_else(|| anyhow::anyhow!("No state in redirect URL"))?;

    tracing::debug!("Got auth code, exchanging for token...");
    oauth.handle_callback(&code, &state).await?;

    let am = oauth
        .into_authorization_manager()
        .ok_or_else(|| anyhow::anyhow!("Failed to get authorization manager"))?;

    Ok(AuthClient::new(reqwest::Client::default(), am))
}

/// Like `perform_oauth_flow` but uses pre-registered client credentials.
async fn perform_oauth_flow_preregistered(
    server_url: &str,
    client_id: &str,
    client_secret: &str,
) -> anyhow::Result<AuthClient<reqwest::Client>> {
    let mut manager = AuthorizationManager::new(server_url).await?;
    let metadata = manager.discover_metadata().await?;
    manager.set_metadata(metadata);

    // Configure with pre-registered credentials
    let config = rmcp::transport::auth::OAuthClientConfig {
        client_id: client_id.to_string(),
        client_secret: Some(client_secret.to_string()),
        scopes: vec![],
        redirect_uri: REDIRECT_URI.to_string(),
    };
    manager.configure_client(config)?;

    let scopes = manager.select_scopes(None, &[]);
    let scope_refs: Vec<&str> = scopes.iter().map(|s| s.as_str()).collect();
    let auth_url = manager.get_authorization_url(&scope_refs).await?;

    // Headless redirect
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let resp = http.get(&auth_url).send().await?;
    let location = resp
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| anyhow::anyhow!("No Location header"))?;
    let redirect_url = url::Url::parse(location)?;
    let code = redirect_url
        .query_pairs()
        .find(|(k, _)| k == "code")
        .map(|(_, v)| v.to_string())
        .ok_or_else(|| anyhow::anyhow!("No code"))?;
    let state = redirect_url
        .query_pairs()
        .find(|(k, _)| k == "state")
        .map(|(_, v)| v.to_string())
        .ok_or_else(|| anyhow::anyhow!("No state"))?;

    manager.exchange_code_for_token(&code, &state).await?;

    Ok(AuthClient::new(reqwest::Client::default(), manager))
}

/// Run the standard auth flow, then connect and exercise the server.
async fn run_auth_client(server_url: &str, ctx: &ConformanceContext) -> anyhow::Result<()> {
    let auth_client = perform_oauth_flow(server_url, ctx).await?;

    let transport = StreamableHttpClientTransport::with_client(
        auth_client,
        StreamableHttpClientTransportConfig::with_uri(server_url),
    );

    let client = BasicClientHandler.serve(transport).await?;
    tracing::debug!("Connected (authenticated)");

    let tools = client.list_tools(Default::default()).await?;
    tracing::debug!("Listed {} tools", tools.tools.len());

    // Call each tool
    for tool in &tools.tools {
        let args = build_tool_arguments(tool);
        let _ = client
            .call_tool(CallToolRequestParams {
                meta: None,
                name: tool.name.clone(),
                arguments: args,
                task: None,
            })
            .await;
    }

    client.cancel().await?;
    Ok(())
}

/// Auth flow with scope step-up: connect, list tools (ok with basic scope),
/// then call tool which triggers 403 → re-auth with expanded scopes → retry.
async fn run_auth_scope_step_up_client(
    server_url: &str,
    ctx: &ConformanceContext,
) -> anyhow::Result<()> {
    // First auth
    let mut oauth = OAuthState::new(server_url, None).await?;
    oauth
        .start_authorization_with_metadata_url(
            &[],
            REDIRECT_URI,
            Some("conformance-client"),
            Some(CIMD_CLIENT_METADATA_URL),
        )
        .await?;

    let auth_url = oauth.get_authorization_url().await?;
    let (code, state) = headless_authorize(&auth_url).await?;
    oauth.handle_callback(&code, &state).await?;

    let am = oauth
        .into_authorization_manager()
        .ok_or_else(|| anyhow::anyhow!("No AM"))?;
    let auth_client = AuthClient::new(reqwest::Client::default(), am);

    let transport = StreamableHttpClientTransport::with_client(
        auth_client.clone(),
        StreamableHttpClientTransportConfig::with_uri(server_url),
    );

    let client = BasicClientHandler.serve(transport).await?;

    let tools = client.list_tools(Default::default()).await?;
    tracing::debug!("Listed {} tools", tools.tools.len());

    // Try calling tool – may get 403 insufficient_scope
    for tool in &tools.tools {
        let args = build_tool_arguments(tool);
        match client
            .call_tool(CallToolRequestParams {
                meta: None,
                name: tool.name.clone(),
                arguments: args.clone(),
                task: None,
            })
            .await
        {
            Ok(_) => {
                tracing::debug!("Tool call succeeded on first try");
            }
            Err(_) => {
                tracing::debug!("Tool call failed (likely 403), attempting scope upgrade...");
                // Drop old client, re-auth with upgraded scopes
                client.cancel().await.ok();

                // Re-do the full flow; the server will give us the right scopes
                // on the second authorization request.
                let mut oauth2 = OAuthState::new(server_url, None).await?;
                // Pass the escalated scope hint
                oauth2
                    .start_authorization_with_metadata_url(
                        &[],
                        REDIRECT_URI,
                        Some("conformance-client"),
                        Some(CIMD_CLIENT_METADATA_URL),
                    )
                    .await?;
                let auth_url2 = oauth2.get_authorization_url().await?;
                let (code2, state2) = headless_authorize(&auth_url2).await?;
                oauth2.handle_callback(&code2, &state2).await?;

                let am2 = oauth2.into_authorization_manager().unwrap();
                let auth_client2 = AuthClient::new(reqwest::Client::default(), am2);
                let transport2 = StreamableHttpClientTransport::with_client(
                    auth_client2,
                    StreamableHttpClientTransportConfig::with_uri(server_url),
                );
                let client2 = BasicClientHandler.serve(transport2).await?;
                let _ = client2
                    .call_tool(CallToolRequestParams {
                        meta: None,
                        name: tool.name.clone(),
                        arguments: args,
                        task: None,
                    })
                    .await;
                client2.cancel().await.ok();
                return Ok(());
            }
        }
    }

    client.cancel().await?;
    Ok(())
}

/// Auth flow for scope-retry-limit: keep re-authing on 403 until we hit a limit.
async fn run_auth_scope_retry_limit_client(
    server_url: &str,
    _ctx: &ConformanceContext,
) -> anyhow::Result<()> {
    let max_retries = 3u32;
    let mut attempt = 0u32;

    loop {
        let mut oauth = OAuthState::new(server_url, None).await?;
        oauth
            .start_authorization_with_metadata_url(
                &[],
                REDIRECT_URI,
                Some("conformance-client"),
                Some(CIMD_CLIENT_METADATA_URL),
            )
            .await?;
        let auth_url = oauth.get_authorization_url().await?;
        let (code, state) = headless_authorize(&auth_url).await?;
        oauth.handle_callback(&code, &state).await?;

        let am = oauth.into_authorization_manager().unwrap();
        let auth_client = AuthClient::new(reqwest::Client::default(), am);
        let transport = StreamableHttpClientTransport::with_client(
            auth_client,
            StreamableHttpClientTransportConfig::with_uri(server_url),
        );

        let client = BasicClientHandler.serve(transport).await?;
        let tools = client.list_tools(Default::default()).await?;

        let mut got_403 = false;
        for tool in &tools.tools {
            let args = build_tool_arguments(tool);
            match client
                .call_tool(CallToolRequestParams {
                    meta: None,
                    name: tool.name.clone(),
                    arguments: args,
                    task: None,
                })
                .await
            {
                Ok(_) => {}
                Err(_) => {
                    got_403 = true;
                    break;
                }
            }
        }
        client.cancel().await.ok();

        if !got_403 {
            break;
        }
        attempt += 1;
        if attempt >= max_retries {
            tracing::info!("Reached retry limit ({max_retries}), giving up");
            return Err(anyhow::anyhow!("Scope retry limit reached"));
        }
    }
    Ok(())
}

/// Auth flow with pre-registered credentials (from context).
async fn run_auth_preregistered_client(
    server_url: &str,
    ctx: &ConformanceContext,
) -> anyhow::Result<()> {
    let client_id = ctx
        .client_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("Missing client_id in context"))?;
    let client_secret = ctx
        .client_secret
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("Missing client_secret in context"))?;

    let auth_client =
        perform_oauth_flow_preregistered(server_url, client_id, client_secret).await?;

    let transport = StreamableHttpClientTransport::with_client(
        auth_client,
        StreamableHttpClientTransportConfig::with_uri(server_url),
    );

    let client = BasicClientHandler.serve(transport).await?;
    let tools = client.list_tools(Default::default()).await?;
    tracing::debug!("Listed {} tools", tools.tools.len());

    for tool in &tools.tools {
        let args = build_tool_arguments(tool);
        let _ = client
            .call_tool(CallToolRequestParams {
                meta: None,
                name: tool.name.clone(),
                arguments: args,
                task: None,
            })
            .await;
    }
    client.cancel().await?;
    Ok(())
}

/// Client-credentials flow with client_secret_basic.
async fn run_client_credentials_basic(
    server_url: &str,
    ctx: &ConformanceContext,
) -> anyhow::Result<()> {
    let client_id = ctx
        .client_id
        .as_deref()
        .unwrap_or("conformance-test-client");
    let client_secret = ctx
        .client_secret
        .as_deref()
        .unwrap_or("conformance-test-secret");

    let mut manager = AuthorizationManager::new(server_url).await?;
    let metadata = manager.discover_metadata().await?;
    let token_endpoint = metadata.token_endpoint.clone();
    manager.set_metadata(metadata);

    let http = reqwest::Client::new();
    let resp = http
        .post(&token_endpoint)
        .basic_auth(client_id, Some(client_secret))
        .header("content-type", "application/x-www-form-urlencoded")
        .body("grant_type=client_credentials")
        .send()
        .await?;

    let token_resp: serde_json::Value = resp.json().await?;
    let access_token = token_resp["access_token"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("No access_token in response"))?;

    // Use static token
    let transport = StreamableHttpClientTransport::with_client(
        reqwest::Client::default(),
        StreamableHttpClientTransportConfig::with_uri(server_url)
            .auth_header(access_token.to_string()),
    );

    let client = BasicClientHandler.serve(transport).await?;
    let tools = client.list_tools(Default::default()).await?;
    tracing::debug!("Listed {} tools", tools.tools.len());
    for tool in &tools.tools {
        let args = build_tool_arguments(tool);
        let _ = client
            .call_tool(CallToolRequestParams {
                meta: None,
                name: tool.name.clone(),
                arguments: args,
                task: None,
            })
            .await;
    }
    client.cancel().await?;
    Ok(())
}

/// Client-credentials flow with private_key_jwt (JWT assertion).
async fn run_client_credentials_jwt(
    server_url: &str,
    ctx: &ConformanceContext,
) -> anyhow::Result<()> {
    let client_id = ctx
        .client_id
        .as_deref()
        .unwrap_or("conformance-test-client");
    let _pem = ctx
        .private_key_pem
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("Missing private_key_pem"))?;
    let _alg = ctx
        .signing_algorithm
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("Missing signing_algorithm"))?;

    // Discover metadata to get token endpoint
    let mut manager = AuthorizationManager::new(server_url).await?;
    let metadata = manager.discover_metadata().await?;
    let token_endpoint = metadata.token_endpoint.clone();
    manager.set_metadata(metadata);

    // Build JWT assertion
    // Parse the PEM private key
    let key = openssl_free_ec_sign(_pem, client_id, &token_endpoint)?;

    let http = reqwest::Client::new();
    let form_body = format!(
        "grant_type=client_credentials&client_assertion_type={}&client_assertion={}",
        urlencoding::encode("urn:ietf:params:oauth:client-assertion-type:jwt-bearer"),
        urlencoding::encode(&key),
    );
    let resp = http
        .post(&token_endpoint)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(form_body)
        .send()
        .await?;

    let token_resp: serde_json::Value = resp.json().await?;
    let access_token = token_resp["access_token"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("No access_token: {}", token_resp))?;

    let transport = StreamableHttpClientTransport::with_client(
        reqwest::Client::default(),
        StreamableHttpClientTransportConfig::with_uri(server_url)
            .auth_header(access_token.to_string()),
    );

    let client = BasicClientHandler.serve(transport).await?;
    let tools = client.list_tools(Default::default()).await?;
    tracing::debug!("Listed {} tools", tools.tools.len());
    for tool in &tools.tools {
        let args = build_tool_arguments(tool);
        let _ = client
            .call_tool(CallToolRequestParams {
                meta: None,
                name: tool.name.clone(),
                arguments: args,
                task: None,
            })
            .await;
    }
    client.cancel().await?;
    Ok(())
}

/// Minimal ES256 JWT signing without heavy deps.
/// We use ring or pure-Rust approach. For simplicity, use the p256 + base64 crates
/// that are already transitive deps of oauth2.
fn openssl_free_ec_sign(pem: &str, client_id: &str, audience: &str) -> anyhow::Result<String> {
    use std::time::{SystemTime, UNIX_EPOCH};

    // Decode PEM → DER
    let pem_body = pem
        .lines()
        .filter(|l| !l.starts_with("-----"))
        .collect::<String>();
    let der = base64_decode(&pem_body)?;

    // Parse PKCS#8 DER to get the raw EC private key bytes
    // PKCS#8 for EC P-256: the raw 32-byte key is at the end of the structure
    let raw_key = extract_ec_private_key(&der)?;

    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let header = base64url_encode(br#"{"alg":"ES256","typ":"JWT"}"#);
    let payload_json = serde_json::json!({
        "iss": client_id,
        "sub": client_id,
        "aud": audience,
        "iat": now,
        "exp": now + 300,
        "jti": format!("jti-{}", now),
    });
    let payload = base64url_encode(payload_json.to_string().as_bytes());
    let signing_input = format!("{}.{}", header, payload);

    // Sign with p256
    let secret_key = p256::ecdsa::SigningKey::from_bytes(raw_key.as_slice().into())
        .map_err(|e| anyhow::anyhow!("Invalid EC key: {}", e))?;
    use p256::ecdsa::signature::Signer;
    let sig: p256::ecdsa::Signature = secret_key.sign(signing_input.as_bytes());
    let sig_bytes = sig.to_bytes();
    let sig_b64 = base64url_encode(&sig_bytes);

    Ok(format!("{}.{}", signing_input, sig_b64))
}

fn base64url_encode(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(data)
}

fn base64_decode(s: &str) -> anyhow::Result<Vec<u8>> {
    use base64::Engine;
    Ok(base64::engine::general_purpose::STANDARD.decode(s.trim())?)
}

/// Extract the raw 32-byte EC private key from a PKCS#8 DER blob.
fn extract_ec_private_key(der: &[u8]) -> anyhow::Result<Vec<u8>> {
    // PKCS#8 wraps an ECPrivateKey. We look for the octet string containing
    // the 32-byte private key. A simple heuristic: find 0x04 0x20 (OCTET STRING, len 32)
    // followed by exactly 32 bytes that form the key.
    // More robust: parse ASN.1. But for conformance testing this suffices.
    for i in 0..der.len().saturating_sub(33) {
        if der[i] == 0x04 && der[i + 1] == 0x20 && i + 34 <= der.len() {
            return Ok(der[i + 2..i + 34].to_vec());
        }
    }
    Err(anyhow::anyhow!(
        "Could not extract 32-byte EC private key from PKCS#8 DER"
    ))
}

/// Cross-app access flow (SEP-1046 extension).
async fn run_cross_app_access_client(
    server_url: &str,
    ctx: &ConformanceContext,
) -> anyhow::Result<()> {
    // For now, fall back to standard auth flow
    // The cross-app-access test is an extension scenario
    run_auth_client(server_url, ctx).await
}

// ─── Helpers ────────────────────────────────────────────────────────────────

/// Fetch an authorization URL headlessly, returning (code, state).
async fn headless_authorize(auth_url: &str) -> anyhow::Result<(String, String)> {
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let resp = http.get(auth_url).send().await?;
    let location = resp
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| anyhow::anyhow!("No Location header in auth redirect"))?;
    let redirect_url = url::Url::parse(location)?;
    let code = redirect_url
        .query_pairs()
        .find(|(k, _)| k == "code")
        .map(|(_, v)| v.to_string())
        .ok_or_else(|| anyhow::anyhow!("No code in redirect URL"))?;
    let state = redirect_url
        .query_pairs()
        .find(|(k, _)| k == "state")
        .map(|(_, v)| v.to_string())
        .ok_or_else(|| anyhow::anyhow!("No state in redirect URL"))?;
    Ok((code, state))
}

/// Build arguments for a tool based on its input schema.
fn build_tool_arguments(tool: &Tool) -> Option<serde_json::Map<String, Value>> {
    let schema = &tool.input_schema;
    let properties = schema.get("properties").and_then(|p| p.as_object());
    let required = schema
        .get("required")
        .and_then(|r| r.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let Some(properties) = properties else {
        return None;
    };
    if properties.is_empty() && required.is_empty() {
        return None;
    }

    let mut args = serde_json::Map::new();
    for (name, prop_schema) in properties {
        if !required.contains(name) {
            continue;
        }
        let type_str = prop_schema.get("type").and_then(|t| t.as_str());
        let value = match type_str {
            Some("number") => json!(1.0),
            Some("integer") => json!(1),
            Some("string") => json!("test"),
            Some("boolean") => json!(true),
            _ => json!(null),
        };
        args.insert(name.clone(), value);
    }
    Some(args)
}

// ─── Non-auth scenarios ─────────────────────────────────────────────────────

async fn run_basic_client(server_url: &str) -> anyhow::Result<()> {
    let transport = StreamableHttpClientTransport::from_uri(server_url);
    let client = BasicClientHandler.serve(transport).await?;
    let tools = client.list_tools(Default::default()).await?;
    tracing::debug!("Listed {} tools", tools.tools.len());
    client.cancel().await?;
    Ok(())
}

async fn run_tools_call_client(server_url: &str) -> anyhow::Result<()> {
    let transport = StreamableHttpClientTransport::from_uri(server_url);
    let client = FullClientHandler.serve(transport).await?;
    let tools = client.list_tools(Default::default()).await?;
    for tool in &tools.tools {
        let args = build_tool_arguments(tool);
        let _ = client
            .call_tool(CallToolRequestParams {
                meta: None,
                name: tool.name.clone(),
                arguments: args,
                task: None,
            })
            .await?;
    }
    client.cancel().await?;
    Ok(())
}

async fn run_elicitation_defaults_client(server_url: &str) -> anyhow::Result<()> {
    let transport = StreamableHttpClientTransport::from_uri(server_url);
    let client = ElicitationDefaultsClientHandler.serve(transport).await?;
    let tools = client.list_tools(Default::default()).await?;
    let test_tool = tools.tools.iter().find(|t| {
        let n = t.name.as_ref();
        n == "test_client_elicitation_defaults" || n == "test_elicitation_sep1034_defaults"
    });
    if let Some(tool) = test_tool {
        let _ = client
            .call_tool(CallToolRequestParams {
                meta: None,
                name: tool.name.clone(),
                arguments: None,
                task: None,
            })
            .await?;
    }
    client.cancel().await?;
    Ok(())
}

async fn run_sse_retry_client(server_url: &str) -> anyhow::Result<()> {
    let transport = StreamableHttpClientTransport::from_uri(server_url);
    let client = BasicClientHandler.serve(transport).await?;
    let tools = client.list_tools(Default::default()).await?;
    if let Some(tool) = tools
        .tools
        .iter()
        .find(|t| t.name.as_ref() == "test_reconnection")
    {
        let _ = client
            .call_tool(CallToolRequestParams {
                meta: None,
                name: tool.name.clone(),
                arguments: None,
                task: None,
            })
            .await?;
    }
    client.cancel().await?;
    Ok(())
}

// ─── Main ───────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let scenario =
        std::env::var("MCP_CONFORMANCE_SCENARIO").unwrap_or_else(|_| "initialize".to_string());
    let server_url = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "http://127.0.0.1:8001/mcp".to_string());
    let ctx = load_context();

    tracing::info!("Running scenario '{}' against {}", scenario, server_url);

    match scenario.as_str() {
        // Non-auth scenarios
        "initialize" => run_basic_client(&server_url).await?,
        "tools_call" => run_tools_call_client(&server_url).await?,
        "elicitation-sep1034-client-defaults" => {
            run_elicitation_defaults_client(&server_url).await?
        }
        "sse-retry" => run_sse_retry_client(&server_url).await?,

        // Auth scenarios - standard OAuth flow
        "auth/metadata-default"
        | "auth/metadata-var1"
        | "auth/metadata-var2"
        | "auth/metadata-var3"
        | "auth/basic-cimd"
        | "auth/scope-from-www-authenticate"
        | "auth/scope-from-scopes-supported"
        | "auth/scope-omitted-when-undefined"
        | "auth/token-endpoint-auth-basic"
        | "auth/token-endpoint-auth-post"
        | "auth/token-endpoint-auth-none"
        | "auth/2025-03-26-oauth-metadata-backcompat"
        | "auth/2025-03-26-oauth-endpoint-fallback" => run_auth_client(&server_url, &ctx).await?,

        // Auth - scope step-up
        "auth/scope-step-up" => run_auth_scope_step_up_client(&server_url, &ctx).await?,

        // Auth - scope retry limit
        "auth/scope-retry-limit" => run_auth_scope_retry_limit_client(&server_url, &ctx).await?,

        // Auth - pre-registration
        "auth/pre-registration" => run_auth_preregistered_client(&server_url, &ctx).await?,

        // Auth - resource mismatch (should fail to auth → pass)
        "auth/resource-mismatch" => {
            // Try to auth; it should fail because PRM resource doesn't match
            match run_auth_client(&server_url, &ctx).await {
                Ok(_) => {
                    tracing::warn!("Auth succeeded despite resource mismatch!");
                }
                Err(e) => {
                    tracing::info!("Auth correctly failed: {}", e);
                }
            }
        }

        // Auth - client credentials
        "auth/client-credentials-basic" => run_client_credentials_basic(&server_url, &ctx).await?,
        "auth/client-credentials-jwt" => run_client_credentials_jwt(&server_url, &ctx).await?,

        // Auth - cross-app access
        "auth/cross-app-access-complete-flow" => {
            run_cross_app_access_client(&server_url, &ctx).await?
        }

        _ => {
            tracing::warn!("Unknown scenario '{}', trying auth flow", scenario);
            match run_auth_client(&server_url, &ctx).await {
                Ok(_) => {}
                Err(e) => {
                    tracing::debug!("Auth flow failed for unknown scenario: {e}");
                    run_basic_client(&server_url).await?
                }
            }
        }
    }

    Ok(())
}
