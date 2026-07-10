#![allow(deprecated)]
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
const CACHE_TTL_MS: u64 = 60_000;

/// Helper to convert a serde_json::Value (must be an object) into a JsonObject
fn json_object(v: Value) -> JsonObject {
    match v {
        Value::Object(map) => map,
        _ => panic!("Expected JSON object"),
    }
}

/// Signing key for SEP-2322 `requestState` sealing. A fixed key is fine for a
/// conformance harness; real servers must load a secret out of clients' reach.
const REQUEST_STATE_KEY: &[u8] = b"rust-sdk-conformance-request-state-key!!";

#[derive(Clone)]
struct ConformanceServer {
    subscriptions: Arc<Mutex<HashSet<String>>>,
    log_level: Arc<Mutex<LoggingLevel>>,
    request_state_codec: RequestStateCodec,
}

impl ConformanceServer {
    fn new() -> Self {
        Self {
            subscriptions: Arc::new(Mutex::new(HashSet::new())),
            log_level: Arc::new(Mutex::new(LoggingLevel::Debug)),
            request_state_codec: RequestStateCodec::new(REQUEST_STATE_KEY),
        }
    }
}

// ─── SEP-2322 MRTR (InputRequiredResult) helpers ────────────────────────────

fn mrtr_elicitation_request(message: &str, properties: Value, required: Value) -> InputRequest {
    InputRequest::Elicitation(ElicitRequest::new(
        ElicitRequestParams::FormElicitationParams {
            meta: None,
            message: message.into(),
            requested_schema: serde_json::from_value(json!({
                "type": "object",
                "properties": properties,
                "required": required,
            }))
            .expect("valid elicitation schema"),
        },
    ))
}

fn mrtr_sampling_request(prompt: &str) -> InputRequest {
    InputRequest::CreateMessage(CreateMessageRequest::new(CreateMessageRequestParams::new(
        vec![SamplingMessage::user_text(prompt)],
        100,
    )))
}

fn mrtr_list_roots_request() -> InputRequest {
    InputRequest::ListRoots(ListRootsRequest::default())
}

/// An input response is usable when it is a JSON object (an `ElicitResult`,
/// `CreateMessageResult`, or `ListRootsResult` shape). Anything else (e.g. a
/// bare number) is treated as missing so the server re-requests it.
fn mrtr_response<'a>(
    responses: Option<&'a InputResponses>,
    key: &str,
) -> Option<&'a serde_json::Map<String, Value>> {
    responses
        .and_then(|r| r.get(key))
        .and_then(Value::as_object)
}

impl ConformanceServer {
    fn mrtr_tampered_state_error() -> ErrorData {
        ErrorData::invalid_params("requestState failed integrity verification", None)
    }

    /// SEP-2322 test tools. Each returns an `InputRequiredResult` until the
    /// client retries with the expected `inputResponses` (and, where used, the
    /// echoed `requestState`).
    ///
    /// `meta` is the request's `_meta`, which the service loop moves out of the
    /// params and into the `RequestContext`.
    async fn call_mrtr_tool(
        &self,
        request: CallToolRequestParams,
        meta: &Meta,
    ) -> Result<CallToolResponse, ErrorData> {
        let responses = request.input_responses.as_ref();
        match request.name.as_ref() {
            "test_input_required_result_elicitation" => {
                match mrtr_response(responses, "user_name") {
                    Some(response) => {
                        let name = response
                            .get("content")
                            .and_then(|c| c.get("name"))
                            .and_then(Value::as_str)
                            .unwrap_or("friend");
                        Ok(CallToolResult::success(vec![ContentBlock::text(format!(
                            "Hello, {name}!"
                        ))])
                        .into())
                    }
                    // Initial call, or a retry with missing/invalid responses:
                    // (re-)request the input per the SEP's recommendation.
                    None => {
                        let mut requests = InputRequests::new();
                        requests.insert(
                            "user_name".into(),
                            mrtr_elicitation_request(
                                "What is your name?",
                                json!({ "name": { "type": "string" } }),
                                json!(["name"]),
                            ),
                        );
                        Ok(InputRequiredResult::from_input_requests(requests).into())
                    }
                }
            }

            "test_input_required_result_sampling" => {
                match mrtr_response(responses, "capital_question") {
                    Some(response) => {
                        let text = response
                            .get("content")
                            .and_then(|c| c.get("text"))
                            .and_then(Value::as_str)
                            .unwrap_or("(no sampling text)");
                        Ok(CallToolResult::success(vec![ContentBlock::text(format!(
                            "Sampling response: {text}"
                        ))])
                        .into())
                    }
                    None => {
                        let mut requests = InputRequests::new();
                        requests.insert(
                            "capital_question".into(),
                            mrtr_sampling_request("What is the capital of France?"),
                        );
                        Ok(InputRequiredResult::from_input_requests(requests).into())
                    }
                }
            }

            "test_input_required_result_list_roots" => {
                match mrtr_response(responses, "client_roots") {
                    Some(response) => {
                        let roots = response
                            .get("roots")
                            .and_then(Value::as_array)
                            .map(|roots| {
                                roots
                                    .iter()
                                    .filter_map(|r| r.get("uri").and_then(Value::as_str))
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            })
                            .unwrap_or_default();
                        Ok(CallToolResult::success(vec![ContentBlock::text(format!(
                            "Client roots: [{roots}]"
                        ))])
                        .into())
                    }
                    None => {
                        let mut requests = InputRequests::new();
                        requests.insert("client_roots".into(), mrtr_list_roots_request());
                        Ok(InputRequiredResult::from_input_requests(requests).into())
                    }
                }
            }

            "test_input_required_result_request_state"
            | "test_input_required_result_tampered_state" => {
                match request.request_state.as_deref() {
                    // Initial call: request confirmation and seal our progress.
                    None => {
                        let sealed = self
                            .request_state_codec
                            .seal_json(&json!({ "stage": "confirm" }))
                            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
                        let mut requests = InputRequests::new();
                        requests.insert(
                            "confirm".into(),
                            mrtr_elicitation_request(
                                "Please confirm",
                                json!({ "ok": { "type": "boolean" } }),
                                json!(["ok"]),
                            ),
                        );
                        Ok(InputRequiredResult::new(Some(requests), Some(sealed)).into())
                    }
                    // Retry: the echoed state is untrusted input and MUST pass
                    // integrity verification before we act on it.
                    Some(sealed) => {
                        self.request_state_codec
                            .open(sealed)
                            .map_err(|_| Self::mrtr_tampered_state_error())?;
                        Ok(
                            CallToolResult::success(vec![ContentBlock::text(
                                "Confirmed: state-ok",
                            )])
                            .into(),
                        )
                    }
                }
            }

            "test_input_required_result_multiple_inputs" => {
                if let Some(sealed) = request.request_state.as_deref() {
                    self.request_state_codec
                        .open(sealed)
                        .map_err(|_| Self::mrtr_tampered_state_error())?;
                }
                let all_present = mrtr_response(responses, "user_name").is_some()
                    && mrtr_response(responses, "greeting").is_some()
                    && mrtr_response(responses, "client_roots").is_some();
                if all_present && request.request_state.is_some() {
                    Ok(
                        CallToolResult::success(vec![ContentBlock::text("All inputs received")])
                            .into(),
                    )
                } else {
                    let sealed = self
                        .request_state_codec
                        .seal_json(&json!({ "stage": "gather" }))
                        .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
                    let mut requests = InputRequests::new();
                    requests.insert(
                        "user_name".into(),
                        mrtr_elicitation_request(
                            "What is your name?",
                            json!({ "name": { "type": "string" } }),
                            json!(["name"]),
                        ),
                    );
                    requests.insert(
                        "greeting".into(),
                        mrtr_sampling_request("Generate a greeting"),
                    );
                    requests.insert("client_roots".into(), mrtr_list_roots_request());
                    Ok(InputRequiredResult::new(Some(requests), Some(sealed)).into())
                }
            }

            "test_input_required_result_multi_round" => {
                let round = match request.request_state.as_deref() {
                    None => 0,
                    Some(sealed) => {
                        let state: Value = self
                            .request_state_codec
                            .open_json(sealed)
                            .map_err(|_| Self::mrtr_tampered_state_error())?;
                        state.get("round").and_then(Value::as_i64).unwrap_or(0)
                    }
                };
                match round {
                    0 => {
                        let sealed = self
                            .request_state_codec
                            .seal_json(&json!({ "round": 1 }))
                            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
                        let mut requests = InputRequests::new();
                        requests.insert(
                            "step1".into(),
                            mrtr_elicitation_request(
                                "Step 1: What is your name?",
                                json!({ "name": { "type": "string" } }),
                                json!(["name"]),
                            ),
                        );
                        Ok(InputRequiredResult::new(Some(requests), Some(sealed)).into())
                    }
                    1 => {
                        let sealed = self
                            .request_state_codec
                            .seal_json(&json!({ "round": 2 }))
                            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
                        let mut requests = InputRequests::new();
                        requests.insert(
                            "step2".into(),
                            mrtr_elicitation_request(
                                "Step 2: What is your favorite color?",
                                json!({ "color": { "type": "string" } }),
                                json!(["color"]),
                            ),
                        );
                        Ok(InputRequiredResult::new(Some(requests), Some(sealed)).into())
                    }
                    _ => Ok(CallToolResult::success(vec![ContentBlock::text(
                        "Multi-round flow complete",
                    )])
                    .into()),
                }
            }

            "test_input_required_result_capabilities" => {
                if responses.is_some() {
                    return Ok(CallToolResult::success(vec![ContentBlock::text(
                        "Capability-aware flow complete",
                    )])
                    .into());
                }
                // Per SEP-2322, only request inputs the client declared support
                // for in `_meta['io.modelcontextprotocol/clientCapabilities']`.
                let capabilities = meta.client_capabilities().unwrap_or_default();
                let mut requests = InputRequests::new();
                if capabilities.elicitation.is_some() {
                    requests.insert(
                        "user_name".into(),
                        mrtr_elicitation_request(
                            "What is your name?",
                            json!({ "name": { "type": "string" } }),
                            json!(["name"]),
                        ),
                    );
                }
                if capabilities.sampling.is_some() {
                    requests.insert(
                        "greeting".into(),
                        mrtr_sampling_request("Generate a greeting"),
                    );
                }
                if capabilities.roots.is_some() {
                    requests.insert("client_roots".into(), mrtr_list_roots_request());
                }
                if requests.is_empty() {
                    Ok(CallToolResult::success(vec![ContentBlock::text(
                        "Client declared no MRTR-capable capabilities",
                    )])
                    .into())
                } else {
                    Ok(InputRequiredResult::from_input_requests(requests).into())
                }
            }

            _ => Err(ErrorData::invalid_params(
                format!("Unknown tool: {}", request.name),
                None,
            )),
        }
    }
}

impl ServerHandler for ConformanceServer {
    async fn initialize(
        &self,
        request: InitializeRequestParams,
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
        .with_protocol_version(request.protocol_version)
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
        // SEP-2322 MRTR test tools; all take no arguments.
        let mrtr_tools = [
            (
                "test_input_required_result_elicitation",
                "Requires an elicitation input via InputRequiredResult (SEP-2322)",
            ),
            (
                "test_input_required_result_sampling",
                "Requires a sampling input via InputRequiredResult (SEP-2322)",
            ),
            (
                "test_input_required_result_list_roots",
                "Requires a roots/list input via InputRequiredResult (SEP-2322)",
            ),
            (
                "test_input_required_result_request_state",
                "Round-trips integrity-protected requestState (SEP-2322)",
            ),
            (
                "test_input_required_result_multiple_inputs",
                "Requires elicitation + sampling + roots inputs in one round (SEP-2322)",
            ),
            (
                "test_input_required_result_multi_round",
                "Drives multiple input_required rounds with evolving requestState (SEP-2322)",
            ),
            (
                "test_input_required_result_tampered_state",
                "Rejects tampered requestState with a JSON-RPC error (SEP-2322)",
            ),
            (
                "test_input_required_result_capabilities",
                "Only requests inputs for declared client capabilities (SEP-2322)",
            ),
        ];
        let tools = tools
            .into_iter()
            .chain(mrtr_tools.into_iter().map(|(name, description)| {
                Tool::new(
                    name,
                    description,
                    json_object(json!({ "type": "object", "properties": {} })),
                )
            }))
            .collect();
        Ok(ListToolsResult {
            tools,
            ..Default::default()
        }
        .with_ttl_ms(CACHE_TTL_MS)
        .with_cache_scope(CacheScope::Public))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        cx: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        if request.name.starts_with("test_input_required_result_") {
            return self.call_mrtr_tool(request, &cx.meta).await;
        }
        let args = request.arguments.unwrap_or_default();
        let result = match request.name.as_ref() {
            "test_simple_text" => Ok(CallToolResult::success(vec![ContentBlock::text(
                "This is a simple text response for testing.",
            )])),

            "test_image_content" => Ok(CallToolResult::success(vec![ContentBlock::image(
                TEST_IMAGE_DATA,
                "image/png",
            )])),

            "test_audio_content" => {
                let audio = ContentBlock::Audio(AudioContent::new(TEST_AUDIO_DATA, "audio/wav"));
                Ok(CallToolResult::success(vec![audio]))
            }

            "test_embedded_resource" => Ok(CallToolResult::success(vec![ContentBlock::resource(
                ResourceContents::TextResourceContents {
                    uri: "test://embedded-resource".into(),
                    mime_type: Some("text/plain".into()),
                    text: "This is an embedded resource content.".into(),
                    meta: None,
                },
            )])),

            "test_multiple_content_types" => Ok(CallToolResult::success(vec![
                ContentBlock::text("Multiple content types test:"),
                ContentBlock::image(TEST_IMAGE_DATA, "image/png"),
                ContentBlock::resource(ResourceContents::TextResourceContents {
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
                        .notify_logging_message(
                            LoggingMessageNotificationParam::new(LoggingLevel::Info, json!(msg))
                                .with_logger("conformance-server"),
                        )
                        .await;
                    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
                }

                Ok(CallToolResult::success(vec![ContentBlock::text(
                    "Logging test completed",
                )]))
            }

            "test_error_handling" => Ok(CallToolResult::error(vec![ContentBlock::text(
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
                            .notify_progress(
                                ProgressNotificationParam::new(token.clone(), progress)
                                    .with_total(100.0)
                                    .with_message(message),
                            )
                            .await;
                    }
                    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
                }

                Ok(CallToolResult::success(vec![ContentBlock::text(
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
                        Ok(CallToolResult::success(vec![ContentBlock::text(format!(
                            "LLM response: {}",
                            text
                        ))]))
                    }
                    Err(e) => Ok(CallToolResult::error(vec![ContentBlock::text(format!(
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
                    .create_elicitation(ElicitRequestParams::FormElicitationParams {
                        meta: None,
                        message: message.into(),
                        requested_schema: schema,
                    })
                    .await
                {
                    Ok(result) => Ok(CallToolResult::success(vec![ContentBlock::text(format!(
                        "User response: action={}, content={:?}",
                        match result.action {
                            ElicitationAction::Accept => "accept",
                            ElicitationAction::Decline => "decline",
                            ElicitationAction::Cancel => "cancel",
                            _ => "unknown",
                        },
                        result.content
                    ))])),
                    Err(e) => Ok(CallToolResult::error(vec![ContentBlock::text(format!(
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
                    .create_elicitation(ElicitRequestParams::FormElicitationParams {
                        meta: None,
                        message: "Please provide values (all have defaults)".into(),
                        requested_schema: schema,
                    })
                    .await
                {
                    Ok(result) => Ok(CallToolResult::success(vec![ContentBlock::text(format!(
                        "Elicitation completed: action={}, content={:?}",
                        match result.action {
                            ElicitationAction::Accept => "accept",
                            ElicitationAction::Decline => "decline",
                            ElicitationAction::Cancel => "cancel",
                            _ => "unknown",
                        },
                        result.content
                    ))])),
                    Err(e) => Ok(CallToolResult::error(vec![ContentBlock::text(format!(
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
                    .create_elicitation(ElicitRequestParams::FormElicitationParams {
                        meta: None,
                        message: "Test enum schema improvements".into(),
                        requested_schema: schema,
                    })
                    .await
                {
                    Ok(result) => Ok(CallToolResult::success(vec![ContentBlock::text(format!(
                        "Enum elicitation completed: action={}",
                        match result.action {
                            ElicitationAction::Accept => "accept",
                            ElicitationAction::Decline => "decline",
                            ElicitationAction::Cancel => "cancel",
                            _ => "unknown",
                        }
                    ))])),
                    Err(e) => Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                        "Elicitation error: {}",
                        e
                    ))])),
                }
            }

            "json_schema_2020_12_tool" => {
                let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("world");
                Ok(CallToolResult::success(vec![ContentBlock::text(format!(
                    "Hello, {}!",
                    name
                ))]))
            }

            "test_reconnection" => {
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                Ok(CallToolResult::success(vec![ContentBlock::text(
                    "Reconnection test completed",
                )]))
            }

            _ => Err(ErrorData::invalid_params(
                format!("Unknown tool: {}", request.name),
                None,
            )),
        };
        result.map(Into::into)
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _cx: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        Ok(ListResourcesResult {
            resources: vec![
                Resource::new("test://static-text", "Static Text Resource")
                    .with_description("A static text resource for testing")
                    .with_mime_type("text/plain"),
                Resource::new("test://static-binary", "Static Binary Resource")
                    .with_description("A static binary/blob resource for testing")
                    .with_mime_type("image/png"),
            ],
            ..Default::default()
        }
        .with_ttl_ms(CACHE_TTL_MS)
        .with_cache_scope(CacheScope::Public))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _cx: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        let uri = request.uri.as_str();
        let result = match uri {
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
                        Some(json!({ "uri": uri })),
                    ))
                }
            }
        };
        result
            .map(|result| {
                result
                    .with_ttl_ms(CACHE_TTL_MS)
                    .with_cache_scope(CacheScope::Public)
            })
            .map(Into::into)
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _cx: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, ErrorData> {
        Ok(ListResourceTemplatesResult {
            resource_templates: vec![
                ResourceTemplate::new("test://template/{id}/data", "Dynamic Resource")
                    .with_description("A dynamic resource with parameter substitution")
                    .with_mime_type("application/json"),
            ],
            ..Default::default()
        }
        .with_ttl_ms(CACHE_TTL_MS)
        .with_cache_scope(CacheScope::Public))
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
                        PromptArgument::new("arg1")
                            .with_description("First test argument")
                            .with_required(true),
                        PromptArgument::new("arg2")
                            .with_description("Second test argument")
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
                Prompt::new(
                    "test_input_required_result_prompt",
                    Some(
                        "A prompt that requires elicitation input via InputRequiredResult (SEP-2322)",
                    ),
                    None,
                ),
            ],
            ..Default::default()
        }
        .with_ttl_ms(CACHE_TTL_MS)
        .with_cache_scope(CacheScope::Public))
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        _cx: RequestContext<RoleServer>,
    ) -> Result<GetPromptResponse, ErrorData> {
        // SEP-2322: InputRequiredResult on a non-tool request (prompts/get).
        if request.name == "test_input_required_result_prompt" {
            return match mrtr_response(request.input_responses.as_ref(), "user_context") {
                Some(response) => {
                    let context = response
                        .get("content")
                        .and_then(|c| c.get("context"))
                        .and_then(Value::as_str)
                        .unwrap_or("(no context)");
                    Ok(GetPromptResult::new(vec![PromptMessage::new_text(
                        Role::User,
                        format!("Prompt with elicited context: {context}"),
                    )])
                    .with_description("A prompt built from elicited context")
                    .into())
                }
                None => {
                    let mut requests = InputRequests::new();
                    requests.insert(
                        "user_context".into(),
                        mrtr_elicitation_request(
                            "What context should the prompt use?",
                            json!({ "context": { "type": "string" } }),
                            json!(["context"]),
                        ),
                    );
                    Ok(InputRequiredResult::from_input_requests(requests).into())
                }
            };
        }
        let result = match request.name.as_str() {
            "test_simple_prompt" => Ok(GetPromptResult::new(vec![PromptMessage::new_text(
                Role::User,
                "This is a simple test prompt.",
            )])
            .with_description("A simple test prompt")),
            "test_prompt_with_arguments" => {
                let args = request.arguments.unwrap_or_default();
                let arg1 = args.get("arg1").and_then(|v| v.as_str()).unwrap_or("");
                let arg2 = args.get("arg2").and_then(|v| v.as_str()).unwrap_or("");
                Ok(GetPromptResult::new(vec![PromptMessage::new_text(
                    Role::User,
                    format!("Prompt with arguments: arg1='{}', arg2='{}'", arg1, arg2),
                )])
                .with_description("A prompt with arguments"))
            }
            "test_prompt_with_embedded_resource" => Ok(GetPromptResult::new(vec![
                PromptMessage::new_text(Role::User, "Here is a resource:"),
                PromptMessage::new_resource(
                    Role::User,
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
                let image_content = ImageContent::new(TEST_IMAGE_DATA, "image/png");
                Ok(GetPromptResult::new(vec![
                    PromptMessage::new_text(Role::User, "Here is an image:"),
                    PromptMessage::new(Role::User, ContentBlock::Image(image_content)),
                ])
                .with_description("A prompt with an image"))
            }
            _ => Err(ErrorData::invalid_params(
                format!("Unknown prompt: {}", request.name),
                None,
            )),
        };
        result.map(Into::into)
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
            _ => vec![],
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
    let stateless = std::env::var_os("STATELESS").is_some();
    let config = StreamableHttpServerConfig::default()
        .with_stateful_mode(!stateless)
        .with_json_response(stateless);
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
