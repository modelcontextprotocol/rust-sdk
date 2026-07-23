use rmcp::{
    ClientHandler, ClientLifecycleMode, ClientServiceExt, ErrorData, RoleClient, ServiceExt,
    model::*,
    service::RequestContext,
    transport::{
        AuthClient, AuthorizationManager, StreamableHttpClientTransport,
        auth::{
            AuthorizationCallback, AuthorizationRequest, ClientCredentialsConfig,
            InMemoryCredentialStore, JwtSigningAlgorithm, OAuthState,
        },
        streamable_http_client::StreamableHttpClientTransportConfig,
    },
};
use serde_json::{Value, json};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

// ─── Context parsed from MCP_CONFORMANCE_CONTEXT ────────────────────────────

#[derive(Debug, Default, serde::Deserialize)]
struct ConformanceToolCall {
    name: String,
    #[serde(default)]
    arguments: Option<serde_json::Map<String, Value>>,
}

#[derive(Debug, Default, serde::Deserialize)]
struct ConformanceContext {
    #[serde(default, alias = "toolCalls")]
    tool_calls: Vec<ConformanceToolCall>,
    #[serde(default)]
    client_id: Option<String>,
    #[serde(default)]
    client_secret: Option<String>,
    // client-credentials-jwt
    #[serde(default)]
    private_key_pem: Option<String>,
    #[serde(default)]
    signing_algorithm: Option<String>,
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
        info.capabilities.elicitation = Some(
            ElicitationCapability::new()
                .with_form(FormElicitationCapability::new().with_schema_validation(true)),
        );
        info
    }

    async fn create_elicitation(
        &self,
        request: ElicitRequestParams,
        _cx: RequestContext<RoleClient>,
    ) -> Result<ElicitResult, ErrorData> {
        let content = match &request {
            ElicitRequestParams::FormElicitationParams {
                requested_schema, ..
            } => {
                let mut defaults = serde_json::Map::new();
                for (name, prop) in &requested_schema.properties {
                    match prop {
                        PrimitiveSchemaDefinition::String(s) => {
                            if let Some(d) = &s.default {
                                defaults.insert(name.clone(), Value::String(d.clone()));
                            }
                        }
                        PrimitiveSchemaDefinition::Number(n) => {
                            if let Some(d) = n.default {
                                defaults.insert(name.clone(), json!(d));
                            }
                        }
                        PrimitiveSchemaDefinition::Integer(i) => {
                            if let Some(d) = i.default {
                                defaults.insert(name.clone(), json!(d));
                            }
                        }
                        PrimitiveSchemaDefinition::Boolean(b) => {
                            if let Some(d) = b.default {
                                defaults.insert(name.clone(), Value::Bool(d));
                            }
                        }
                        PrimitiveSchemaDefinition::Enum(e) => {
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
                                            d.iter().map(|s| Value::String(s.clone())).collect(),
                                        )
                                    })
                                }
                                EnumSchema::Multi(MultiSelectEnumSchema::Titled(t)) => {
                                    t.default.as_ref().map(|d| {
                                        Value::Array(
                                            d.iter().map(|s| Value::String(s.clone())).collect(),
                                        )
                                    })
                                }
                                EnumSchema::Legacy(_) => None,
                                _ => None,
                            };
                            if let Some(v) = val {
                                defaults.insert(name.clone(), v);
                            }
                        }
                        _ => {}
                    }
                }
                Some(Value::Object(defaults))
            }
            _ => Some(json!({})),
        };
        let mut result = ElicitResult::new(ElicitationAction::Accept);
        if let Some(c) = content {
            result = result.with_content(c);
        }
        Ok(result)
    }
}

/// A client handler that handles both sampling and elicitation
struct FullClientHandler;

impl ClientHandler for FullClientHandler {
    fn get_info(&self) -> ClientInfo {
        let mut info = ClientInfo::default();
        info.capabilities.elicitation = Some(
            ElicitationCapability::new()
                .with_form(FormElicitationCapability::new().with_schema_validation(true)),
        );
        info
    }

    async fn create_message(
        &self,
        params: CreateMessageRequestParams,
        _cx: RequestContext<RoleClient>,
    ) -> Result<CreateMessageResult, ErrorData> {
        let prompt_text = params
            .messages
            .first()
            .and_then(|m| m.content.first())
            .and_then(|c| c.as_text())
            .map(|t| t.text.clone())
            .unwrap_or_default();
        Ok(CreateMessageResult::new(
            SamplingMessage::new(
                Role::Assistant,
                SamplingMessageContentBlock::text(format!(
                    "This is a mock LLM response to: {}",
                    prompt_text
                )),
            ),
            "mock-model".into(),
        )
        .with_stop_reason("endTurn"))
    }

    async fn create_elicitation(
        &self,
        _request: ElicitRequestParams,
        _cx: RequestContext<RoleClient>,
    ) -> Result<ElicitResult, ErrorData> {
        Ok(ElicitResult::new(ElicitationAction::Accept)
            .with_content(json!({"username": "testuser", "email": "test@example.com"})))
    }
}

// ─── OAuth helpers ──────────────────────────────────────────────────────────

const CIMD_CLIENT_METADATA_URL: &str = "https://conformance-test.local/client-metadata.json";
const REDIRECT_URI: &str = "http://localhost:3000/callback";
const SCOPE_STEP_UP_INITIAL_SCOPES: &[&str] = &["mcp:basic"];
const SCOPE_STEP_UP_ESCALATED_SCOPES: &[&str] = &["mcp:basic", "mcp:write"];

/// Perform the headless OAuth authorization-code flow.
///
/// 1. Discover metadata, register (or use CIMD), get auth URL
/// 2. Fetch the auth URL with redirect:manual → extract code from Location header
/// 3. Exchange code for token
/// 4. Return an `AuthClient` wrapping `reqwest::Client`
async fn perform_oauth_flow(
    server_url: &str,
    _ctx: &ConformanceContext,
) -> anyhow::Result<AuthClient<reqwest::Client>> {
    let mut oauth = OAuthState::new(server_url, None).await?;

    // Discover + register + get auth URL
    oauth
        .start_authorization(
            AuthorizationRequest::new(REDIRECT_URI)
                .with_client_name("conformance-client")
                .with_client_metadata_url(CIMD_CLIENT_METADATA_URL),
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

    let callback = AuthorizationCallback::from_redirect_url(location)?;

    tracing::debug!("Got auth code, exchanging for token...");
    oauth
        .handle_callback_with_issuer(
            &callback.code,
            &callback.csrf_token,
            callback.issuer.as_deref(),
        )
        .await?;

    let am = oauth
        .into_authorization_manager()
        .ok_or_else(|| anyhow::anyhow!("Failed to get authorization manager"))?;

    Ok(AuthClient::new(reqwest::Client::default(), am))
}

/// Like `perform_oauth_flow` but uses pre-registered client credentials,
/// exercising the SDK's high-level `OAuthState` path (no DCR).
async fn perform_oauth_flow_preregistered(
    server_url: &str,
    client_id: &str,
    client_secret: &str,
) -> anyhow::Result<AuthClient<reqwest::Client>> {
    let mut oauth = OAuthState::new(server_url, None).await?;

    oauth
        .start_authorization(
            AuthorizationRequest::new(REDIRECT_URI)
                .with_preregistered_client(client_id)
                .with_client_secret(client_secret),
        )
        .await?;

    let auth_url = oauth.get_authorization_url().await?;
    let callback = headless_authorize(&auth_url).await?;
    oauth
        .handle_callback_with_issuer(
            &callback.code,
            &callback.csrf_token,
            callback.issuer.as_deref(),
        )
        .await?;

    let am = oauth
        .into_authorization_manager()
        .ok_or_else(|| anyhow::anyhow!("Failed to get authorization manager"))?;

    Ok(AuthClient::new(reqwest::Client::default(), am))
}

/// Run the standard auth flow, then connect and exercise the server.
async fn run_auth_client(server_url: &str, ctx: &ConformanceContext) -> anyhow::Result<()> {
    let auth_client = perform_oauth_flow(server_url, ctx).await?;

    let transport = StreamableHttpClientTransport::with_client(
        auth_client,
        StreamableHttpClientTransportConfig::with_uri(server_url),
    );

    // The 2026-07-28 auth mocks require the modern per-request lifecycle
    // (MCP-Protocol-Version header on every request), so negotiate via the
    // discover lifecycle rather than the legacy initialize handshake.
    let client = BasicClientHandler
        .serve_with_lifecycle(
            transport,
            ClientLifecycleMode::Discover {
                preferred_versions: preferred_protocol_versions(),
            },
        )
        .await?;
    tracing::debug!("Connected (authenticated)");

    let tools = client.list_tools(Default::default()).await?;
    tracing::debug!("Listed {} tools", tools.tools.len());

    // Call each tool
    for tool in &tools.tools {
        let args = build_tool_arguments(tool);
        let _ = client
            .call_tool(call_tool_params(tool.name.clone(), args))
            .await;
    }

    client.cancel().await?;
    Ok(())
}

/// Auth flow with scope step-up: connect, list tools (ok with basic scope),
/// then call tool which triggers 403 → re-auth with expanded scopes → retry.
async fn run_auth_scope_step_up_client(
    server_url: &str,
    _ctx: &ConformanceContext,
) -> anyhow::Result<()> {
    let mut oauth = OAuthState::new(server_url, None).await?;
    oauth
        .start_authorization(
            AuthorizationRequest::new(REDIRECT_URI)
                .with_scopes(SCOPE_STEP_UP_INITIAL_SCOPES.iter().copied())
                .with_client_name("conformance-client")
                .with_client_metadata_url(CIMD_CLIENT_METADATA_URL),
        )
        .await?;

    let auth_url = oauth.get_authorization_url().await?;
    let callback = headless_authorize(&auth_url).await?;
    oauth
        .handle_callback_with_issuer(
            &callback.code,
            &callback.csrf_token,
            callback.issuer.as_deref(),
        )
        .await?;

    let am = oauth
        .into_authorization_manager()
        .ok_or_else(|| anyhow::anyhow!("No AM"))?;
    let auth_client = AuthClient::new(reqwest::Client::default(), am);

    let transport = StreamableHttpClientTransport::with_client(
        auth_client.clone(),
        StreamableHttpClientTransportConfig::with_uri(server_url),
    );

    let client = BasicClientHandler
        .serve_with_lifecycle(transport, conformance_lifecycle())
        .await?;

    let tools = client.list_tools(Default::default()).await?;
    tracing::debug!("Listed {} tools", tools.tools.len());

    // Try calling tool – may get 403 insufficient_scope
    for tool in &tools.tools {
        let args = build_tool_arguments(tool);
        match client
            .call_tool(call_tool_params(tool.name.clone(), args.clone()))
            .await
        {
            Ok(_) => {
                tracing::debug!("Tool call succeeded on first try");
            }
            Err(_) => {
                tracing::debug!("Tool call failed (likely 403), attempting scope upgrade...");
                // Drop old client, re-auth with upgraded scopes
                client.cancel().await.ok();

                let mut oauth2 = OAuthState::new(server_url, None).await?;
                oauth2
                    .start_authorization(
                        AuthorizationRequest::new(REDIRECT_URI)
                            .with_scopes(SCOPE_STEP_UP_ESCALATED_SCOPES.iter().copied())
                            .with_client_name("conformance-client")
                            .with_client_metadata_url(CIMD_CLIENT_METADATA_URL),
                    )
                    .await?;
                let auth_url2 = oauth2.get_authorization_url().await?;
                let callback2 = headless_authorize(&auth_url2).await?;
                oauth2
                    .handle_callback_with_issuer(
                        &callback2.code,
                        &callback2.csrf_token,
                        callback2.issuer.as_deref(),
                    )
                    .await?;

                let am2 = oauth2.into_authorization_manager().ok_or_else(|| {
                    anyhow::anyhow!("Missing authorization manager after step-up")
                })?;
                let auth_client2 = AuthClient::new(reqwest::Client::default(), am2);
                let transport2 = StreamableHttpClientTransport::with_client(
                    auth_client2,
                    StreamableHttpClientTransportConfig::with_uri(server_url),
                );
                let client2 = BasicClientHandler
                    .serve_with_lifecycle(transport2, conformance_lifecycle())
                    .await?;
                client2
                    .call_tool(call_tool_params(tool.name.clone(), args))
                    .await?;
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
            .start_authorization(
                AuthorizationRequest::new(REDIRECT_URI)
                    .with_client_name("conformance-client")
                    .with_client_metadata_url(CIMD_CLIENT_METADATA_URL),
            )
            .await?;
        let auth_url = oauth.get_authorization_url().await?;
        let callback = headless_authorize(&auth_url).await?;
        oauth
            .handle_callback_with_issuer(
                &callback.code,
                &callback.csrf_token,
                callback.issuer.as_deref(),
            )
            .await?;

        let am = oauth
            .into_authorization_manager()
            .ok_or_else(|| anyhow::anyhow!("Missing authorization manager"))?;
        let auth_client = AuthClient::new(reqwest::Client::default(), am);
        let transport = StreamableHttpClientTransport::with_client(
            auth_client,
            StreamableHttpClientTransportConfig::with_uri(server_url),
        );

        let client = BasicClientHandler.serve(transport).await?;
        let tools = match client.list_tools(Default::default()).await {
            Ok(tools) => tools,
            Err(err) => {
                tracing::info!(
                    "Scope retry limit scenario stopped after authorization attempt {}: {}",
                    attempt + 1,
                    err
                );
                client.cancel().await.ok();
                return Ok(());
            }
        };

        let mut got_403 = false;
        for tool in &tools.tools {
            let args = build_tool_arguments(tool);
            match client
                .call_tool(call_tool_params(tool.name.clone(), args))
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
            return Ok(());
        }
    }
    Ok(())
}

async fn migration_token(
    server_url: &str,
    store: &InMemoryCredentialStore,
) -> anyhow::Result<String> {
    let mut manager = AuthorizationManager::new(server_url).await?;
    manager.set_credential_store(store.clone());

    if manager.initialize_from_store().await? {
        return Ok(manager.get_access_token().await?);
    }

    let metadata = manager.discover_metadata().await?;
    manager.set_metadata(metadata);
    manager
        .register_client("conformance-client", REDIRECT_URI, &[])
        .await?;

    let scopes = manager.select_scopes(None, &[]);
    let scope_refs: Vec<&str> = scopes.iter().map(|s| s.as_str()).collect();
    let auth_url = manager.get_authorization_url(&scope_refs).await?;
    let callback = headless_authorize(&auth_url).await?;
    manager
        .exchange_code_for_token_with_issuer(
            &callback.code,
            &callback.csrf_token,
            callback.issuer.as_deref(),
        )
        .await?;

    Ok(manager.get_access_token().await?)
}

async fn run_auth_server_migration_client(
    server_url: &str,
    _ctx: &ConformanceContext,
) -> anyhow::Result<()> {
    let store = InMemoryCredentialStore::new();
    let http = reqwest::Client::new();
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}});

    let mut token = migration_token(server_url, &store).await?;
    for _ in 0..3 {
        let resp = http
            .post(server_url)
            .header(
                "MCP-Protocol-Version",
                conformance_protocol_version().as_str(),
            )
            .bearer_auth(&token)
            .json(&body)
            .send()
            .await?;
        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            token = migration_token(server_url, &store).await?;
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
            .call_tool(call_tool_params(tool.name.clone(), args))
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
            .call_tool(call_tool_params(tool.name.clone(), args))
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
        .clone()
        .unwrap_or_else(|| "conformance-test-client".to_string());
    let signing_key = ctx
        .private_key_pem
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Missing private_key_pem"))?
        .as_bytes()
        .to_vec();
    let signing_algorithm = match ctx
        .signing_algorithm
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("Missing signing_algorithm"))?
    {
        "RS256" => JwtSigningAlgorithm::RS256,
        "RS384" => JwtSigningAlgorithm::RS384,
        "RS512" => JwtSigningAlgorithm::RS512,
        "ES256" => JwtSigningAlgorithm::ES256,
        "ES384" => JwtSigningAlgorithm::ES384,
        algorithm => anyhow::bail!("Unsupported signing_algorithm: {algorithm}"),
    };
    let config = ClientCredentialsConfig::PrivateKeyJwt {
        client_id,
        signing_key,
        signing_algorithm,
        token_endpoint_audience: None,
        scopes: vec![],
        resource: Some(server_url.to_string()),
    };
    let mut oauth_state = OAuthState::new(server_url, None).await?;
    oauth_state.authenticate_client_credentials(config).await?;
    let manager = oauth_state
        .into_authorization_manager()
        .ok_or_else(|| anyhow::anyhow!("Client credentials flow did not authorize"))?;

    let transport = StreamableHttpClientTransport::with_client(
        AuthClient::new(reqwest::Client::default(), manager),
        StreamableHttpClientTransportConfig::with_uri(server_url),
    );

    let client = BasicClientHandler.serve(transport).await?;
    let tools = client.list_tools(Default::default()).await?;
    tracing::debug!("Listed {} tools", tools.tools.len());
    for tool in &tools.tools {
        let args = build_tool_arguments(tool);
        let _ = client
            .call_tool(call_tool_params(tool.name.clone(), args))
            .await;
    }
    client.cancel().await?;
    Ok(())
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

/// Fetch an authorization URL headlessly, returning the callback parameters.
async fn headless_authorize(auth_url: &str) -> anyhow::Result<AuthorizationCallback> {
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let resp = http.get(auth_url).send().await?;
    let location = resp
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| anyhow::anyhow!("No Location header in auth redirect"))?;
    AuthorizationCallback::from_redirect_url(location).map_err(Into::into)
}

/// Build a `CallToolRequestParams` for a tool, optionally with arguments.
fn call_tool_params(
    name: std::borrow::Cow<'static, str>,
    arguments: Option<serde_json::Map<String, Value>>,
) -> CallToolRequestParams {
    let mut p = CallToolRequestParams::new(name);
    if let Some(a) = arguments {
        p = p.with_arguments(a);
    }
    p
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

    let properties = properties?;
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

async fn run_tools_call_client(server_url: &str, ctx: &ConformanceContext) -> anyhow::Result<()> {
    run_tools_call_client_with_lifecycle(server_url, ctx, conformance_lifecycle()).await
}

async fn run_discover_tools_call_client(
    server_url: &str,
    ctx: &ConformanceContext,
) -> anyhow::Result<()> {
    run_tools_call_client_with_lifecycle(
        server_url,
        ctx,
        ClientLifecycleMode::Discover {
            preferred_versions: preferred_protocol_versions(),
        },
    )
    .await
}

async fn run_tools_call_client_with_lifecycle(
    server_url: &str,
    ctx: &ConformanceContext,
    lifecycle: ClientLifecycleMode,
) -> anyhow::Result<()> {
    let transport = StreamableHttpClientTransport::from_uri(server_url);
    let client = FullClientHandler
        .serve_with_lifecycle(transport, lifecycle)
        .await?;
    let tools = client.list_tools(Default::default()).await?;

    if ctx.tool_calls.is_empty() {
        for tool in &tools.tools {
            let args = build_tool_arguments(tool);
            client
                .call_tool(call_tool_params(tool.name.clone(), args))
                .await?;
        }
    } else {
        for tool_call in &ctx.tool_calls {
            client
                .call_tool(call_tool_params(
                    tool_call.name.clone().into(),
                    tool_call.arguments.clone(),
                ))
                .await?;
        }
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
            .call_tool(call_tool_params(tool.name.clone(), None))
            .await?;
    }
    client.cancel().await?;
    Ok(())
}

fn conformance_protocol_version() -> ProtocolVersion {
    std::env::var("MCP_CONFORMANCE_PROTOCOL_VERSION")
        .ok()
        .and_then(|version| serde_json::from_value(Value::String(version)).ok())
        .unwrap_or(ProtocolVersion::V_2025_11_25)
}

fn conformance_lifecycle() -> ClientLifecycleMode {
    if conformance_protocol_version().as_str() >= ProtocolVersion::V_2026_07_28.as_str() {
        ClientLifecycleMode::Discover {
            preferred_versions: preferred_protocol_versions(),
        }
    } else {
        ClientLifecycleMode::Initialize
    }
}

/// Preferred protocol versions for discover-lifecycle negotiation: the
/// runner-provided version first, then all other known versions newest-first.
fn preferred_protocol_versions() -> Vec<ProtocolVersion> {
    let mut preferred_versions = vec![conformance_protocol_version()];
    for version in ProtocolVersion::KNOWN_VERSIONS.iter().rev() {
        if !preferred_versions.contains(version) {
            preferred_versions.push(version.clone());
        }
    }
    preferred_versions
}

/// Runs draft stateless scenarios through the public discover lifecycle and
/// Streamable HTTP transport.
async fn run_discover_client(server_url: &str) -> anyhow::Result<()> {
    let preferred_versions = preferred_protocol_versions();
    let transport = StreamableHttpClientTransport::from_uri(server_url);
    let client = FullClientHandler
        .serve_with_lifecycle(
            transport,
            ClientLifecycleMode::Discover { preferred_versions },
        )
        .await?;

    let tools = client.list_tools(Default::default()).await?;
    tracing::debug!("Listed {} tools", tools.tools.len());
    for tool in &tools.tools {
        let result = client
            .call_tool(CallToolRequestParams::new(tool.name.clone()))
            .await;
        tracing::debug!("Called {}: {:?}", tool.name, result.is_ok());
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
            .call_tool(call_tool_params(tool.name.clone(), None))
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

    // Safety net: some harness servers intentionally misbehave (e.g. reply
    // with an id-less error instead of answering a request), which would
    // leave the client waiting forever. Exit on our own before the harness's
    // 30s client timeout so it never has to kill us (which has been observed
    // to wedge the harness process in CI).
    let timeout_secs: u64 = std::env::var("MCP_CONFORMANCE_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(25);
    tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs),
        run_scenario(&scenario, &server_url, &ctx),
    )
    .await
    .map_err(|_| anyhow::anyhow!("Scenario '{scenario}' timed out after {timeout_secs}s"))??;

    Ok(())
}

async fn run_scenario(
    scenario: &str,
    server_url: &str,
    ctx: &ConformanceContext,
) -> anyhow::Result<()> {
    match scenario {
        // Non-auth scenarios
        "initialize" => run_basic_client(server_url).await?,
        // SEP-2106: the scenario serves a tool whose schema carries a network
        // `$ref`; the check passes when the client lists tools without
        // dereferencing (fetching) that URL. A plain connect → list_tools →
        // close is sufficient; the scenario's mock server does not implement
        // the discover lifecycle, so `run_discover_client` hangs against it.
        "json-schema-ref-no-deref" => run_basic_client(server_url).await?,
        "tools_call" => run_tools_call_client(server_url, ctx).await?,
        "elicitation-sep1034-client-defaults" => {
            run_elicitation_defaults_client(server_url).await?
        }
        "sse-retry" => run_sse_retry_client(server_url).await?,
        "request-metadata" | "sep-2322-client-request-state" => {
            run_discover_client(server_url).await?
        }
        "http-standard-headers" | "http-custom-headers" | "http-invalid-tool-headers" => {
            run_discover_tools_call_client(server_url, ctx).await?
        }

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
        | "auth/2025-03-26-oauth-endpoint-fallback"
        // Offline access scope handling: positive/negative variants both run
        // the well-behaved flow; the referee inspects the requested scopes.
        | "auth/offline-access-scope"
        | "auth/offline-access-not-supported"
        // SEP-2468 (RFC 9207 iss / RFC 8414 §3.3 issuer-echo). The client
        // captures `iss` from the authorization redirect and passes it to the
        // callback handler; the SDK validates internally. Positive scenarios
        // proceed to the token endpoint; negative scenarios error out (the
        // referee sets `allowClientError`).
        | "auth/iss-supported"
        | "auth/iss-not-advertised"
        | "auth/iss-supported-missing"
        | "auth/iss-wrong-issuer"
        | "auth/iss-unexpected"
        | "auth/iss-normalized"
        | "auth/metadata-issuer-mismatch" => run_auth_client(server_url, ctx).await?,

        // Auth - scope step-up
        "auth/scope-step-up" => run_auth_scope_step_up_client(server_url, ctx).await?,

        // Auth - scope retry limit
        "auth/scope-retry-limit" => run_auth_scope_retry_limit_client(server_url, ctx).await?,

        // Auth - authorization server migration (SEP-2352)
        "auth/authorization-server-migration" => {
            run_auth_server_migration_client(server_url, ctx).await?
        }

        // Auth - pre-registration
        "auth/pre-registration" => run_auth_preregistered_client(server_url, ctx).await?,

        // Auth - resource mismatch (should fail to auth → pass)
        "auth/resource-mismatch" => {
            // Try to auth; it should fail because PRM resource doesn't match
            match run_auth_client(server_url, ctx).await {
                Ok(_) => {
                    tracing::warn!("Auth succeeded despite resource mismatch!");
                }
                Err(e) => {
                    tracing::info!("Auth correctly failed: {}", e);
                }
            }
        }

        // Auth - client credentials
        "auth/client-credentials-basic" => run_client_credentials_basic(server_url, ctx).await?,
        "auth/client-credentials-jwt" => run_client_credentials_jwt(server_url, ctx).await?,

        // Auth - cross-app access
        "auth/cross-app-access-complete-flow" => {
            run_cross_app_access_client(server_url, ctx).await?
        }

        unknown => anyhow::bail!("Unsupported conformance scenario: {unknown}"),
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conformance_context_accepts_camel_case_tool_calls() {
        let context: ConformanceContext = serde_json::from_value(json!({
            "toolCalls": [{
                "name": "test_custom_headers",
                "arguments": { "region": "us-west1" }
            }]
        }))
        .expect("valid conformance context");

        assert_eq!(
            context.tool_calls.first().map(|tool_call| (
                tool_call.name.as_str(),
                tool_call
                    .arguments
                    .as_ref()
                    .and_then(|arguments| arguments.get("region")),
            )),
            Some(("test_custom_headers", Some(&json!("us-west1"))))
        );
    }
}
