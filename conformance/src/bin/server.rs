use std::{collections::HashSet, sync::Arc};

use rmcp::{
    ErrorData, RoleServer, ServerHandler,
    model::*,
    service::RequestContext,
    transport::{
        StreamableHttpServerConfig, StreamableHttpService,
        streamable_http_server::session::local::LocalSessionManager,
    },
};
use serde_json::{Value, json};
use tokio::sync::Mutex;
use tracing_subscriber::EnvFilter;

// Small base64-encoded 1x1 red PNG
const TEST_IMAGE_DATA: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8/5+hHgAHggJ/PchI7wAAAABJRU5ErkJggg==";
// Small base64-encoded WAV (silence)
const TEST_AUDIO_DATA: &str = "UklGRiQAAABXQVZFZm10IBAAAAABAAEARKwAAIhYAQACABAAZGF0YQAAAAA=";

/// Helper to convert a serde_json::Value (must be an object) into a JsonObject
fn json_object(v: Value) -> JsonObject {
    match v {
        Value::Object(map) => map,
        _ => panic!("Expected JSON object"),
    }
}

#[derive(Clone)]
struct ConformanceServer {
    subscriptions: Arc<Mutex<HashSet<String>>>,
    log_level: Arc<Mutex<LoggingLevel>>,
}

impl ConformanceServer {
    fn new() -> Self {
        Self {
            subscriptions: Arc::new(Mutex::new(HashSet::new())),
            log_level: Arc::new(Mutex::new(LoggingLevel::Debug)),
        }
    }
}

impl ServerHandler for ConformanceServer {
    async fn initialize(
        &self,
        _request: InitializeRequestParams,
        _cx: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, ErrorData> {
        Ok(InitializeResult::new(
            ServerCapabilities::builder()
                .enable_prompts()
                .enable_resources()
                .enable_tools()
                .enable_logging()
                .build(),
        )
        .with_server_info(Implementation::new("rust-conformance-server", "0.1.0"))
        .with_instructions("Rust MCP conformance test server"))
    }

    async fn ping(&self, _cx: RequestContext<RoleServer>) -> Result<(), ErrorData> {
        Ok(())
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _cx: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let tools = vec![
            Tool::new(
                "test_simple_text",
                "Returns simple text content",
                json_object(json!({
                    "type": "object",
                    "properties": {}
                })),
            ),
            Tool::new(
                "test_image_content",
                "Returns image content",
                json_object(json!({
                    "type": "object",
                    "properties": {}
                })),
            ),
            Tool::new(
                "test_audio_content",
                "Returns audio content",
                json_object(json!({
                    "type": "object",
                    "properties": {}
                })),
            ),
            Tool::new(
                "test_embedded_resource",
                "Returns embedded resource content",
                json_object(json!({
                    "type": "object",
                    "properties": {}
                })),
            ),
            Tool::new(
                "test_multiple_content_types",
                "Returns multiple content types",
                json_object(json!({
                    "type": "object",
                    "properties": {}
                })),
            ),
            Tool::new(
                "test_tool_with_logging",
                "Sends logging notifications during execution",
                json_object(json!({
                    "type": "object",
                    "properties": {}
                })),
            ),
            Tool::new(
                "test_error_handling",
                "Always returns an error",
                json_object(json!({
                    "type": "object",
                    "properties": {}
                })),
            ),
            Tool::new(
                "test_tool_with_progress",
                "Reports progress notifications",
                json_object(json!({
                    "type": "object",
                    "properties": {}
                })),
            ),
            Tool::new(
                "test_sampling",
                "Requests LLM sampling from client",
                json_object(json!({
                    "type": "object",
                    "properties": {
                        "prompt": { "type": "string", "description": "The prompt to send" }
                    },
                    "required": ["prompt"]
                })),
            ),
            Tool::new(
                "test_elicitation",
                "Requests user input from client",
                json_object(json!({
                    "type": "object",
                    "properties": {
                        "message": { "type": "string", "description": "The message to show" }
                    },
                    "required": ["message"]
                })),
            ),
            Tool::new(
                "test_elicitation_sep1034_defaults",
                "Tests elicitation with default values (SEP-1034)",
                json_object(json!({
                    "type": "object",
                    "properties": {}
                })),
            ),
            Tool::new(
                "test_elicitation_sep1330_enums",
                "Tests enum schema improvements (SEP-1330)",
                json_object(json!({
                    "type": "object",
                    "properties": {}
                })),
            ),
            Tool::new(
                "json_schema_2020_12_tool",
                "Tool with JSON Schema 2020-12 features",
                json_object(json!({
                    "$schema": "https://json-schema.org/draft/2020-12/schema",
                    "type": "object",
                    "$defs": {
                        "address": {
                            "type": "object",
                            "properties": {
                                "street": { "type": "string" },
                                "city": { "type": "string" }
                            }
                        }
                    },
                    "properties": {
                        "name": { "type": "string" },
                        "address": { "$ref": "#/$defs/address" }
                    },
                    "additionalProperties": false
                })),
            ),
            Tool::new(
                "test_reconnection",
                "Tests SSE reconnection behavior",
                json_object(json!({
                    "type": "object",
                    "properties": {}
                })),
            ),
        ];
        Ok(ListToolsResult {
            meta: None,
            tools,
            next_cursor: None,
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        cx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let args = request.arguments.unwrap_or_default();
        match request.name.as_ref() {
            "test_simple_text" => Ok(CallToolResult::success(vec![Content::text(
                "This is a simple text response for testing.",
            )])),

            "test_image_content" => Ok(CallToolResult::success(vec![Content::image(
                TEST_IMAGE_DATA,
                "image/png",
            )])),

            "test_audio_content" => {
                let audio = RawContent::Audio(RawAudioContent {
                    data: TEST_AUDIO_DATA.into(),
                    mime_type: "audio/wav".into(),
                })
                .no_annotation();
                Ok(CallToolResult::success(vec![audio]))
            }

            "test_embedded_resource" => Ok(CallToolResult::success(vec![Content::resource(
                ResourceContents::TextResourceContents {
                    uri: "test://embedded-resource".into(),
                    mime_type: Some("text/plain".into()),
                    text: "This is an embedded resource content.".into(),
                    meta: None,
                },
            )])),

            "test_multiple_content_types" => Ok(CallToolResult::success(vec![
                Content::text("Multiple content types test:"),
                Content::image(TEST_IMAGE_DATA, "image/png"),
                Content::resource(ResourceContents::TextResourceContents {
                    uri: "test://mixed-content-resource".into(),
                    mime_type: Some("application/json".into()),
                    text: r#"{"test":"data","value":123}"#.into(),
                    meta: None,
                }),
            ])),

            "test_tool_with_logging" => {
                for msg in [
                    "Tool execution started",
                    "Tool processing data",
                    "Tool execution completed",
                ] {
                    let _ = cx
                        .peer
                        .notify_logging_message(LoggingMessageNotificationParam {
                            level: LoggingLevel::Info,
                            logger: Some("conformance-server".into()),
                            data: json!(msg),
                        })
                        .await;
                    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
                }

                Ok(CallToolResult::success(vec![Content::text(
                    "Logging test completed",
                )]))
            }

            "test_error_handling" => Ok(CallToolResult::error(vec![Content::text(
                "This tool intentionally returns an error for testing",
            )])),

            "test_tool_with_progress" => {
                let progress_token = cx.meta.get_progress_token();

                for (progress, message) in
                    [(0.0, "Starting"), (50.0, "Halfway"), (100.0, "Complete")]
                {
                    if let Some(token) = &progress_token {
                        let _ = cx
                            .peer
                            .notify_progress(ProgressNotificationParam {
                                progress_token: token.clone(),
                                progress,
                                total: Some(100.0),
                                message: Some(message.into()),
                            })
                            .await;
                    }
                    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
                }

                Ok(CallToolResult::success(vec![Content::text(
                    "Progress test completed",
                )]))
            }

            "test_sampling" => {
                let prompt = args
                    .get("prompt")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Hello");

                match cx
                    .peer
                    .create_message(CreateMessageRequestParams::new(
                        vec![SamplingMessage::user_text(prompt)],
                        100,
                    ))
                    .await
                {
                    Ok(result) => {
                        let text = result
                            .message
                            .content
                            .first()
                            .and_then(|c| c.as_text())
                            .map(|t| t.text.clone())
                            .unwrap_or_else(|| "No text response".into());
                        Ok(CallToolResult::success(vec![Content::text(format!(
                            "LLM response: {}",
                            text
                        ))]))
                    }
                    Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                        "Sampling error: {}",
                        e
                    ))])),
                }
            }

            "test_elicitation" => {
                let message = args
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Please provide your information");

                let schema_json = json!({
                    "type": "object",
                    "properties": {
                        "username": {
                            "type": "string",
                            "description": "User's response"
                        },
                        "email": {
                            "type": "string",
                            "description": "User's email address"
                        }
                    },
                    "required": ["username", "email"]
                });

                let schema: ElicitationSchema = serde_json::from_value(schema_json).unwrap();

                match cx
                    .peer
                    .create_elicitation(CreateElicitationRequestParams::FormElicitationParams {
                        meta: None,
                        message: message.into(),
                        requested_schema: schema,
                    })
                    .await
                {
                    Ok(result) => Ok(CallToolResult::success(vec![Content::text(format!(
                        "User response: action={}, content={:?}",
                        match result.action {
                            ElicitationAction::Accept => "accept",
                            ElicitationAction::Decline => "decline",
                            ElicitationAction::Cancel => "cancel",
                        },
                        result.content
                    ))])),
                    Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                        "Elicitation error: {}",
                        e
                    ))])),
                }
            }

            "test_elicitation_sep1034_defaults" => {
                let schema_json = json!({
                    "type": "object",
                    "properties": {
                        "name": {
                            "type": "string",
                            "description": "User's name",
                            "default": "John Doe"
                        },
                        "age": {
                            "type": "integer",
                            "description": "User's age",
                            "default": 30
                        },
                        "score": {
                            "type": "number",
                            "description": "User's score",
                            "default": 95.5
                        },
                        "status": {
                            "type": "string",
                            "description": "User's status",
                            "enum": ["active", "inactive", "pending"],
                            "default": "active"
                        },
                        "verified": {
                            "type": "boolean",
                            "description": "Whether user is verified",
                            "default": true
                        }
                    }
                });

                let schema: ElicitationSchema = serde_json::from_value(schema_json).unwrap();

                match cx
                    .peer
                    .create_elicitation(CreateElicitationRequestParams::FormElicitationParams {
                        meta: None,
                        message: "Please provide values (all have defaults)".into(),
                        requested_schema: schema,
                    })
                    .await
                {
                    Ok(result) => Ok(CallToolResult::success(vec![Content::text(format!(
                        "Elicitation completed: action={}, content={:?}",
                        match result.action {
                            ElicitationAction::Accept => "accept",
                            ElicitationAction::Decline => "decline",
                            ElicitationAction::Cancel => "cancel",
                        },
                        result.content
                    ))])),
                    Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                        "Elicitation error: {}",
                        e
                    ))])),
                }
            }

            "test_elicitation_sep1330_enums" => {
                let schema_json = json!({
                    "type": "object",
                    "properties": {
                        "untitledSingle": {
                            "type": "string",
                            "enum": ["option1", "option2", "option3"]
                        },
                        "titledSingle": {
                            "type": "string",
                            "oneOf": [
                                { "const": "value1", "title": "First Option" },
                                { "const": "value2", "title": "Second Option" },
                                { "const": "value3", "title": "Third Option" }
                            ]
                        },
                        "legacyEnum": {
                            "type": "string",
                            "enum": ["opt1", "opt2", "opt3"],
                            "enumNames": ["Option One", "Option Two", "Option Three"]
                        },
                        "untitledMulti": {
                            "type": "array",
                            "items": {
                                "type": "string",
                                "enum": ["option1", "option2", "option3"]
                            }
                        },
                        "titledMulti": {
                            "type": "array",
                            "items": {
                                "anyOf": [
                                    { "const": "value1", "title": "First Choice" },
                                    { "const": "value2", "title": "Second Choice" },
                                    { "const": "value3", "title": "Third Choice" }
                                ]
                            }
                        }
                    }
                });

                let schema: ElicitationSchema = serde_json::from_value(schema_json).unwrap();

                match cx
                    .peer
                    .create_elicitation(CreateElicitationRequestParams::FormElicitationParams {
                        meta: None,
                        message: "Test enum schema improvements".into(),
                        requested_schema: schema,
                    })
                    .await
                {
                    Ok(result) => Ok(CallToolResult::success(vec![Content::text(format!(
                        "Enum elicitation completed: action={}",
                        match result.action {
                            ElicitationAction::Accept => "accept",
                            ElicitationAction::Decline => "decline",
                            ElicitationAction::Cancel => "cancel",
                        }
                    ))])),
                    Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                        "Elicitation error: {}",
                        e
                    ))])),
                }
            }

            "json_schema_2020_12_tool" => {
                let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("world");
                Ok(CallToolResult::success(vec![Content::text(format!(
                    "Hello, {}!",
                    name
                ))]))
            }

            "test_reconnection" => {
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                Ok(CallToolResult::success(vec![Content::text(
                    "Reconnection test completed",
                )]))
            }

            _ => Err(ErrorData::invalid_params(
                format!("Unknown tool: {}", request.name),
                None,
            )),
        }
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _cx: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        Ok(ListResourcesResult {
            meta: None,
            resources: vec![
                RawResource {
                    uri: "test://static-text".into(),
                    name: "Static Text Resource".into(),
                    title: None,
                    description: Some("A static text resource for testing".into()),
                    mime_type: Some("text/plain".into()),
                    size: None,
                    icons: None,
                    meta: None,
                }
                .no_annotation(),
                RawResource {
                    uri: "test://static-binary".into(),
                    name: "Static Binary Resource".into(),
                    title: None,
                    description: Some("A static binary/blob resource for testing".into()),
                    mime_type: Some("image/png".into()),
                    size: None,
                    icons: None,
                    meta: None,
                }
                .no_annotation(),
            ],
            next_cursor: None,
        })
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _cx: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, ErrorData> {
        let uri = request.uri.as_str();
        match uri {
            "test://static-text" => Ok(ReadResourceResult::new(vec![
                ResourceContents::TextResourceContents {
                    uri: uri.into(),
                    mime_type: Some("text/plain".into()),
                    text: "This is the content of the static text resource.".into(),
                    meta: None,
                },
            ])),
            "test://static-binary" => Ok(ReadResourceResult::new(vec![
                ResourceContents::BlobResourceContents {
                    uri: uri.into(),
                    mime_type: Some("image/png".into()),
                    blob: TEST_IMAGE_DATA.into(),
                    meta: None,
                },
            ])),
            _ => {
                if uri.starts_with("test://template/") && uri.ends_with("/data") {
                    let id = uri
                        .strip_prefix("test://template/")
                        .and_then(|s| s.strip_suffix("/data"))
                        .unwrap_or("unknown");
                    Ok(ReadResourceResult::new(vec![
                        ResourceContents::TextResourceContents {
                            uri: uri.into(),
                            mime_type: Some("application/json".into()),
                            text: format!(
                                r#"{{"id":"{}","templateTest":true,"data":"Data for ID: {}"}}"#,
                                id, id
                            ),
                            meta: None,
                        },
                    ]))
                } else {
                    Err(ErrorData::resource_not_found(
                        format!("Resource not found: {}", uri),
                        None,
                    ))
                }
            }
        }
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _cx: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, ErrorData> {
        Ok(ListResourceTemplatesResult {
            meta: None,
            resource_templates: vec![
                RawResourceTemplate {
                    uri_template: "test://template/{id}/data".into(),
                    name: "Dynamic Resource".into(),
                    title: None,
                    description: Some("A dynamic resource with parameter substitution".into()),
                    mime_type: Some("application/json".into()),
                    icons: None,
                }
                .no_annotation(),
            ],
            next_cursor: None,
        })
    }

    async fn subscribe(
        &self,
        request: SubscribeRequestParams,
        _cx: RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        let mut subs = self.subscriptions.lock().await;
        subs.insert(request.uri.to_string());
        Ok(())
    }

    async fn unsubscribe(
        &self,
        request: UnsubscribeRequestParams,
        _cx: RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        let mut subs = self.subscriptions.lock().await;
        subs.remove(request.uri.as_str());
        Ok(())
    }

    async fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        _cx: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, ErrorData> {
        Ok(ListPromptsResult {
            meta: None,
            prompts: vec![
                Prompt::new(
                    "test_simple_prompt",
                    Some("A simple test prompt with no arguments"),
                    None,
                ),
                Prompt::new(
                    "test_prompt_with_arguments",
                    Some("A test prompt that accepts arguments"),
                    Some(vec![
                        PromptArgument::new("name")
                            .with_description("The name to greet")
                            .with_required(true),
                        PromptArgument::new("style")
                            .with_description("The greeting style")
                            .with_required(false),
                    ]),
                ),
                Prompt::new(
                    "test_prompt_with_embedded_resource",
                    Some("A test prompt that includes an embedded resource"),
                    None,
                ),
                Prompt::new(
                    "test_prompt_with_image",
                    Some("A test prompt that includes an image"),
                    None,
                ),
            ],
            next_cursor: None,
        })
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        _cx: RequestContext<RoleServer>,
    ) -> Result<GetPromptResult, ErrorData> {
        match request.name.as_str() {
            "test_simple_prompt" => Ok(GetPromptResult::new(vec![PromptMessage::new_text(
                PromptMessageRole::User,
                "This is a simple test prompt.",
            )])
            .with_description("A simple test prompt")),
            "test_prompt_with_arguments" => {
                let args = request.arguments.unwrap_or_default();
                let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("World");
                let style = args
                    .get("style")
                    .and_then(|v| v.as_str())
                    .unwrap_or("friendly");
                Ok(GetPromptResult::new(vec![PromptMessage::new_text(
                    PromptMessageRole::User,
                    format!("Please greet {} in a {} style.", name, style),
                )])
                .with_description("A prompt with arguments"))
            }
            "test_prompt_with_embedded_resource" => Ok(GetPromptResult::new(vec![
                PromptMessage::new_text(PromptMessageRole::User, "Here is a resource:"),
                PromptMessage::new_resource(
                    PromptMessageRole::User,
                    "test://static-text".into(),
                    Some("text/plain".into()),
                    Some("Resource content for prompt".into()),
                    None,
                    None,
                    None,
                ),
            ])
            .with_description("A prompt with an embedded resource")),
            "test_prompt_with_image" => {
                let image_content = RawImageContent {
                    data: TEST_IMAGE_DATA.into(),
                    mime_type: "image/png".into(),
                    meta: None,
                };
                Ok(GetPromptResult::new(vec![
                    PromptMessage::new_text(PromptMessageRole::User, "Here is an image:"),
                    PromptMessage::new(
                        PromptMessageRole::User,
                        PromptMessageContent::Image {
                            image: image_content.no_annotation(),
                        },
                    ),
                ])
                .with_description("A prompt with an image"))
            }
            _ => Err(ErrorData::invalid_params(
                format!("Unknown prompt: {}", request.name),
                None,
            )),
        }
    }

    async fn complete(
        &self,
        request: CompleteRequestParams,
        _cx: RequestContext<RoleServer>,
    ) -> Result<CompleteResult, ErrorData> {
        let values = match &request.r#ref {
            Reference::Resource(_) => {
                if request.argument.name == "id" {
                    vec!["1".into(), "2".into(), "3".into()]
                } else {
                    vec![]
                }
            }
            Reference::Prompt(prompt_ref) => {
                if request.argument.name == "name" {
                    vec!["Alice".into(), "Bob".into(), "Charlie".into()]
                } else if request.argument.name == "style" {
                    vec!["friendly".into(), "formal".into(), "casual".into()]
                } else {
                    vec![prompt_ref.name.clone()]
                }
            }
        };
        Ok(CompleteResult::new(
            CompletionInfo::new(values).map_err(|e| ErrorData::internal_error(e, None))?,
        ))
    }

    async fn set_level(
        &self,
        request: SetLevelRequestParams,
        _cx: RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        let mut level = self.log_level.lock().await;
        *level = request.level;
        Ok(())
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive(tracing::Level::INFO.into()))
        .init();

    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8001);

    let bind_addr = format!("127.0.0.1:{}", port);
    tracing::info!("Starting conformance server on {}", bind_addr);

    let server = ConformanceServer::new();
    let config = StreamableHttpServerConfig::default();
    let service = StreamableHttpService::new(
        move || Ok(server.clone()),
        LocalSessionManager::default().into(),
        config,
    );

    let router = axum::Router::new().nest_service("/mcp", service);

    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    tracing::info!("Conformance server listening on http://{}/mcp", bind_addr);
    axum::serve(listener, router).await?;

    Ok(())
}
