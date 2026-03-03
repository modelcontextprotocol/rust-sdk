use std::{collections::HashSet, future::Future, sync::Arc};

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
    fn initialize(
        &self,
        _request: InitializeRequestParams,
        _cx: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<InitializeResult, ErrorData>> + Send + '_ {
        async {
            Ok(InitializeResult {
                server_info: Implementation {
                    name: "rust-conformance-server".into(),
                    title: None,
                    version: "0.1.0".into(),
                    description: None,
                    icons: None,
                    website_url: None,
                },
                capabilities: ServerCapabilities::builder()
                    .enable_prompts()
                    .enable_resources()
                    .enable_tools()
                    .enable_logging()
                    .build(),
                instructions: Some("Rust MCP conformance test server".into()),
                ..Default::default()
            })
        }
    }

    fn ping(
        &self,
        _cx: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<(), ErrorData>> + Send + '_ {
        async { Ok(()) }
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _cx: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListToolsResult, ErrorData>> + Send + '_ {
        async {
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
    }

    fn call_tool(
        &self,
        request: CallToolRequestParams,
        cx: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<CallToolResult, ErrorData>> + Send + '_ {
        async move {
            let args = request.arguments.unwrap_or_default();
            match request.name.as_ref() {
                "test_simple_text" => Ok(CallToolResult {
                    content: vec![Content::text("This is a simple text response for testing.")],
                    structured_content: None,
                    is_error: None,
                    meta: None,
                }),

                "test_image_content" => Ok(CallToolResult {
                    content: vec![Content::image(TEST_IMAGE_DATA, "image/png")],
                    structured_content: None,
                    is_error: None,
                    meta: None,
                }),

                "test_audio_content" => {
                    // No Content::audio() helper, construct manually
                    let audio = RawContent::Audio(RawAudioContent {
                        data: TEST_AUDIO_DATA.into(),
                        mime_type: "audio/wav".into(),
                    })
                    .no_annotation();
                    Ok(CallToolResult {
                        content: vec![audio],
                        structured_content: None,
                        is_error: None,
                        meta: None,
                    })
                }

                "test_embedded_resource" => Ok(CallToolResult {
                    content: vec![Content::resource(ResourceContents::TextResourceContents {
                        uri: "test://embedded-resource".into(),
                        mime_type: Some("text/plain".into()),
                        text: "This is an embedded resource content.".into(),
                        meta: None,
                    })],
                    structured_content: None,
                    is_error: None,
                    meta: None,
                }),

                "test_multiple_content_types" => Ok(CallToolResult {
                    content: vec![
                        Content::text("Multiple content types test:"),
                        Content::image(TEST_IMAGE_DATA, "image/png"),
                        Content::resource(ResourceContents::TextResourceContents {
                            uri: "test://mixed-content-resource".into(),
                            mime_type: Some("application/json".into()),
                            text: r#"{"test":"data","value":123}"#.into(),
                            meta: None,
                        }),
                    ],
                    structured_content: None,
                    is_error: None,
                    meta: None,
                }),

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

                    Ok(CallToolResult {
                        content: vec![Content::text("Logging test completed")],
                        structured_content: None,
                        is_error: None,
                        meta: None,
                    })
                }

                "test_error_handling" => Ok(CallToolResult {
                    content: vec![Content::text(
                        "This tool intentionally returns an error for testing",
                    )],
                    structured_content: None,
                    is_error: Some(true),
                    meta: None,
                }),

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

                    Ok(CallToolResult {
                        content: vec![Content::text("Progress test completed")],
                        structured_content: None,
                        is_error: None,
                        meta: None,
                    })
                }

                "test_sampling" => {
                    let prompt = args
                        .get("prompt")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Hello");

                    match cx
                        .peer
                        .create_message(CreateMessageRequestParams {
                            meta: None,
                            task: None,
                            messages: vec![SamplingMessage::user_text(prompt)],
                            max_tokens: 100,
                            model_preferences: None,
                            system_prompt: None,
                            include_context: None,
                            temperature: None,
                            stop_sequences: None,
                            metadata: None,
                            tools: None,
                            tool_choice: None,
                        })
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
                            Ok(CallToolResult {
                                content: vec![Content::text(format!("LLM response: {}", text))],
                                structured_content: None,
                                is_error: None,
                                meta: None,
                            })
                        }
                        Err(e) => Ok(CallToolResult {
                            content: vec![Content::text(format!("Sampling error: {}", e))],
                            structured_content: None,
                            is_error: Some(true),
                            meta: None,
                        }),
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
                        Ok(result) => Ok(CallToolResult {
                            content: vec![Content::text(format!(
                                "User response: action={}, content={:?}",
                                match result.action {
                                    ElicitationAction::Accept => "accept",
                                    ElicitationAction::Decline => "decline",
                                    ElicitationAction::Cancel => "cancel",
                                },
                                result.content
                            ))],
                            structured_content: None,
                            is_error: None,
                            meta: None,
                        }),
                        Err(e) => Ok(CallToolResult {
                            content: vec![Content::text(format!("Elicitation error: {}", e))],
                            structured_content: None,
                            is_error: Some(true),
                            meta: None,
                        }),
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
                        Ok(result) => Ok(CallToolResult {
                            content: vec![Content::text(format!(
                                "Elicitation completed: action={}, content={:?}",
                                match result.action {
                                    ElicitationAction::Accept => "accept",
                                    ElicitationAction::Decline => "decline",
                                    ElicitationAction::Cancel => "cancel",
                                },
                                result.content
                            ))],
                            structured_content: None,
                            is_error: None,
                            meta: None,
                        }),
                        Err(e) => Ok(CallToolResult {
                            content: vec![Content::text(format!("Elicitation error: {}", e))],
                            structured_content: None,
                            is_error: Some(true),
                            meta: None,
                        }),
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
                        Ok(result) => Ok(CallToolResult {
                            content: vec![Content::text(format!(
                                "Enum elicitation completed: action={}",
                                match result.action {
                                    ElicitationAction::Accept => "accept",
                                    ElicitationAction::Decline => "decline",
                                    ElicitationAction::Cancel => "cancel",
                                }
                            ))],
                            structured_content: None,
                            is_error: None,
                            meta: None,
                        }),
                        Err(e) => Ok(CallToolResult {
                            content: vec![Content::text(format!("Elicitation error: {}", e))],
                            structured_content: None,
                            is_error: Some(true),
                            meta: None,
                        }),
                    }
                }

                "json_schema_2020_12_tool" => {
                    let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("world");
                    Ok(CallToolResult {
                        content: vec![Content::text(format!("Hello, {}!", name))],
                        structured_content: None,
                        is_error: None,
                        meta: None,
                    })
                }

                "test_reconnection" => {
                    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                    Ok(CallToolResult {
                        content: vec![Content::text("Reconnection test completed")],
                        structured_content: None,
                        is_error: None,
                        meta: None,
                    })
                }

                _ => Err(ErrorData::invalid_params(
                    format!("Unknown tool: {}", request.name),
                    None,
                )),
            }
        }
    }

    fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _cx: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListResourcesResult, ErrorData>> + Send + '_ {
        async {
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
    }

    fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _cx: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ReadResourceResult, ErrorData>> + Send + '_ {
        async move {
            let uri = request.uri.as_str();
            match uri {
                "test://static-text" => Ok(ReadResourceResult {
                    contents: vec![ResourceContents::TextResourceContents {
                        uri: uri.into(),
                        mime_type: Some("text/plain".into()),
                        text: "This is the content of the static text resource.".into(),
                        meta: None,
                    }],
                }),
                "test://static-binary" => Ok(ReadResourceResult {
                    contents: vec![ResourceContents::BlobResourceContents {
                        uri: uri.into(),
                        mime_type: Some("image/png".into()),
                        blob: TEST_IMAGE_DATA.into(),
                        meta: None,
                    }],
                }),
                _ => {
                    // Check if it matches template: test://template/{id}/data
                    if uri.starts_with("test://template/") && uri.ends_with("/data") {
                        let id = uri
                            .strip_prefix("test://template/")
                            .and_then(|s| s.strip_suffix("/data"))
                            .unwrap_or("unknown");
                        Ok(ReadResourceResult {
                            contents: vec![ResourceContents::TextResourceContents {
                                uri: uri.into(),
                                mime_type: Some("application/json".into()),
                                text: format!(
                                    r#"{{"id":"{}","templateTest":true,"data":"Data for ID: {}"}}"#,
                                    id, id
                                ),
                                meta: None,
                            }],
                        })
                    } else {
                        Err(ErrorData::resource_not_found(
                            format!("Resource not found: {}", uri),
                            None,
                        ))
                    }
                }
            }
        }
    }

    fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _cx: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListResourceTemplatesResult, ErrorData>> + Send + '_ {
        async {
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
    }

    fn subscribe(
        &self,
        request: SubscribeRequestParams,
        _cx: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<(), ErrorData>> + Send + '_ {
        async move {
            let mut subs = self.subscriptions.lock().await;
            subs.insert(request.uri.to_string());
            Ok(())
        }
    }

    fn unsubscribe(
        &self,
        request: UnsubscribeRequestParams,
        _cx: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<(), ErrorData>> + Send + '_ {
        async move {
            let mut subs = self.subscriptions.lock().await;
            subs.remove(request.uri.as_str());
            Ok(())
        }
    }

    fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        _cx: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListPromptsResult, ErrorData>> + Send + '_ {
        async {
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
                            PromptArgument {
                                name: "name".into(),
                                title: None,
                                description: Some("The name to greet".into()),
                                required: Some(true),
                            },
                            PromptArgument {
                                name: "style".into(),
                                title: None,
                                description: Some("The greeting style".into()),
                                required: Some(false),
                            },
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
    }

    fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        _cx: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<GetPromptResult, ErrorData>> + Send + '_ {
        async move {
            match request.name.as_str() {
                "test_simple_prompt" => Ok(GetPromptResult {
                    description: Some("A simple test prompt".into()),
                    messages: vec![PromptMessage::new_text(
                        PromptMessageRole::User,
                        "This is a simple test prompt.",
                    )],
                }),
                "test_prompt_with_arguments" => {
                    let args = request.arguments.unwrap_or_default();
                    let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("World");
                    let style = args
                        .get("style")
                        .and_then(|v| v.as_str())
                        .unwrap_or("friendly");
                    Ok(GetPromptResult {
                        description: Some("A prompt with arguments".into()),
                        messages: vec![PromptMessage::new_text(
                            PromptMessageRole::User,
                            format!("Please greet {} in a {} style.", name, style),
                        )],
                    })
                }
                "test_prompt_with_embedded_resource" => Ok(GetPromptResult {
                    description: Some("A prompt with an embedded resource".into()),
                    messages: vec![
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
                    ],
                }),
                "test_prompt_with_image" => {
                    let image_content = RawImageContent {
                        data: TEST_IMAGE_DATA.into(),
                        mime_type: "image/png".into(),
                        meta: None,
                    };
                    Ok(GetPromptResult {
                        description: Some("A prompt with an image".into()),
                        messages: vec![
                            PromptMessage::new_text(PromptMessageRole::User, "Here is an image:"),
                            PromptMessage {
                                role: PromptMessageRole::User,
                                content: PromptMessageContent::Image {
                                    image: image_content.no_annotation(),
                                },
                            },
                        ],
                    })
                }
                _ => Err(ErrorData::invalid_params(
                    format!("Unknown prompt: {}", request.name),
                    None,
                )),
            }
        }
    }

    fn complete(
        &self,
        request: CompleteRequestParams,
        _cx: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<CompleteResult, ErrorData>> + Send + '_ {
        async move {
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
            Ok(CompleteResult {
                completion: CompletionInfo::new(values)
                    .map_err(|e| ErrorData::internal_error(e, None))?,
            })
        }
    }

    fn set_level(
        &self,
        request: SetLevelRequestParams,
        _cx: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<(), ErrorData>> + Send + '_ {
        async move {
            let mut level = self.log_level.lock().await;
            *level = request.level;
            Ok(())
        }
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
    let config = StreamableHttpServerConfig {
        stateful_mode: true,
        ..Default::default()
    };
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
