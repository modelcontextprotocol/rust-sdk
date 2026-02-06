//cargo test --test test_sampling --features "client server"

mod common;

use anyhow::Result;
use common::handlers::{TestClientHandler, TestServer};
use rmcp::{
    ServiceExt,
    model::*,
    service::{RequestContext, Service},
};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn test_basic_sampling_message_creation() -> Result<()> {
    let message = SamplingMessage::user_text("What is the capital of France?");

    let json = serde_json::to_string(&message)?;
    let deserialized: SamplingMessage = serde_json::from_str(&json)?;
    assert_eq!(message, deserialized);
    assert_eq!(message.role, Role::User);

    Ok(())
}

#[tokio::test]
async fn test_sampling_request_params() -> Result<()> {
    let params = CreateMessageRequestParams {
        meta: None,
        task: None,
        messages: vec![SamplingMessage::user_text("Hello, world!")],
        model_preferences: Some(ModelPreferences {
            hints: Some(vec![ModelHint {
                name: Some("claude".to_string()),
            }]),
            cost_priority: Some(0.5),
            speed_priority: Some(0.8),
            intelligence_priority: Some(0.7),
        }),
        system_prompt: Some("You are a helpful assistant.".to_string()),
        temperature: Some(0.7),
        max_tokens: 100,
        stop_sequences: Some(vec!["STOP".to_string()]),
        include_context: Some(ContextInclusion::None),
        metadata: Some(serde_json::json!({"test": "value"})),
        tools: None,
        tool_choice: None,
    };

    let json = serde_json::to_string(&params)?;
    let deserialized: CreateMessageRequestParams = serde_json::from_str(&json)?;
    assert_eq!(params, deserialized);

    assert_eq!(params.messages.len(), 1);
    assert_eq!(params.max_tokens, 100);
    assert_eq!(params.temperature, Some(0.7));

    Ok(())
}

#[tokio::test]
async fn test_sampling_result_structure() -> Result<()> {
    let result = CreateMessageResult {
        message: SamplingMessage::assistant_text("The capital of France is Paris."),
        model: "test-model".to_string(),
        stop_reason: Some(CreateMessageResult::STOP_REASON_END_TURN.to_string()),
    };

    let json = serde_json::to_string(&result)?;
    let deserialized: CreateMessageResult = serde_json::from_str(&json)?;
    assert_eq!(result, deserialized);

    assert_eq!(result.message.role, Role::Assistant);
    assert_eq!(result.model, "test-model");
    assert_eq!(
        result.stop_reason,
        Some(CreateMessageResult::STOP_REASON_END_TURN.to_string())
    );

    Ok(())
}

#[tokio::test]
async fn test_sampling_context_inclusion_enum() -> Result<()> {
    let test_cases = vec![
        (ContextInclusion::None, "none"),
        (ContextInclusion::ThisServer, "thisServer"),
        (ContextInclusion::AllServers, "allServers"),
    ];

    for (context, expected_json) in test_cases {
        let json = serde_json::to_string(&context)?;
        assert_eq!(json, format!("\"{}\"", expected_json));

        let deserialized: ContextInclusion = serde_json::from_str(&json)?;
        assert_eq!(context, deserialized);
    }

    Ok(())
}

#[tokio::test]
async fn test_sampling_integration_with_test_handlers() -> Result<()> {
    let (server_transport, client_transport) = tokio::io::duplex(4096);

    // Start server
    let server_handle = tokio::spawn(async move {
        let server = TestServer::new().serve(server_transport).await?;
        server.waiting().await?;
        anyhow::Ok(())
    });

    // Start client that honors sampling requests
    let handler = TestClientHandler::new(true, true);
    let client = handler.clone().serve(client_transport).await?;

    // Wait for initialization
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Test sampling with context inclusion
    let request = ServerRequest::CreateMessageRequest(CreateMessageRequest {
        method: Default::default(),
        params: CreateMessageRequestParams {
            meta: None,
            task: None,
            messages: vec![SamplingMessage::user_text("What is the capital of France?")],
            include_context: Some(ContextInclusion::ThisServer),
            model_preferences: Some(ModelPreferences {
                hints: Some(vec![ModelHint {
                    name: Some("test-model".to_string()),
                }]),
                cost_priority: Some(0.5),
                speed_priority: Some(0.8),
                intelligence_priority: Some(0.7),
            }),
            system_prompt: Some("You are a helpful assistant.".to_string()),
            temperature: Some(0.7),
            max_tokens: 100,
            stop_sequences: None,
            metadata: None,
            tools: None,
            tool_choice: None,
        },
        extensions: Default::default(),
    });

    let result = handler
        .handle_request(
            request.clone(),
            RequestContext {
                peer: client.peer().clone(),
                ct: CancellationToken::new(),
                id: NumberOrString::Number(1),
                meta: Default::default(),
                extensions: Default::default(),
            },
        )
        .await?;

    // Verify the response
    if let ClientResult::CreateMessageResult(result) = result {
        assert_eq!(result.message.role, Role::Assistant);
        assert_eq!(result.model, "test-model");
        assert_eq!(
            result.stop_reason,
            Some(CreateMessageResult::STOP_REASON_END_TURN.to_string())
        );

        let response_text = result
            .message
            .content
            .first()
            .unwrap()
            .as_text()
            .unwrap()
            .text
            .as_str();
        assert!(
            response_text.contains("test context"),
            "Response should include context for ThisServer inclusion"
        );
    } else {
        panic!("Expected CreateMessageResult");
    }

    client.cancel().await?;
    server_handle.await??;
    Ok(())
}

#[tokio::test]
async fn test_sampling_no_context_inclusion() -> Result<()> {
    let (server_transport, client_transport) = tokio::io::duplex(4096);

    // Start server
    let server_handle = tokio::spawn(async move {
        let server = TestServer::new().serve(server_transport).await?;
        server.waiting().await?;
        anyhow::Ok(())
    });

    // Start client that honors sampling requests
    let handler = TestClientHandler::new(true, true);
    let client = handler.clone().serve(client_transport).await?;

    // Wait for initialization
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Test sampling without context inclusion
    let request = ServerRequest::CreateMessageRequest(CreateMessageRequest {
        method: Default::default(),
        params: CreateMessageRequestParams {
            meta: None,
            task: None,
            messages: vec![SamplingMessage::user_text("Hello")],
            include_context: Some(ContextInclusion::None),
            model_preferences: None,
            system_prompt: None,
            temperature: None,
            max_tokens: 50,
            stop_sequences: None,
            metadata: None,
            tools: None,
            tool_choice: None,
        },
        extensions: Default::default(),
    });

    let result = handler
        .handle_request(
            request.clone(),
            RequestContext {
                peer: client.peer().clone(),
                ct: CancellationToken::new(),
                id: NumberOrString::Number(2),
                meta: Default::default(),
                extensions: Default::default(),
            },
        )
        .await?;

    // Verify the response
    if let ClientResult::CreateMessageResult(result) = result {
        assert_eq!(result.message.role, Role::Assistant);
        assert_eq!(result.model, "test-model");

        let response_text = result
            .message
            .content
            .first()
            .unwrap()
            .as_text()
            .unwrap()
            .text
            .as_str();
        assert!(
            !response_text.contains("test context"),
            "Response should not include context for None inclusion"
        );
    } else {
        panic!("Expected CreateMessageResult");
    }

    client.cancel().await?;
    server_handle.await??;
    Ok(())
}

#[tokio::test]
async fn test_sampling_error_invalid_message_sequence() -> Result<()> {
    let (server_transport, client_transport) = tokio::io::duplex(4096);

    // Start server
    let server_handle = tokio::spawn(async move {
        let server = TestServer::new().serve(server_transport).await?;
        server.waiting().await?;
        anyhow::Ok(())
    });

    // Start client
    let handler = TestClientHandler::new(true, true);
    let client = handler.clone().serve(client_transport).await?;

    // Wait for initialization
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Test sampling with no user messages (should fail)
    let request = ServerRequest::CreateMessageRequest(CreateMessageRequest {
        method: Default::default(),
        params: CreateMessageRequestParams {
            meta: None,
            task: None,
            messages: vec![SamplingMessage::assistant_text(
                "I'm an assistant message without a user message",
            )],
            include_context: Some(ContextInclusion::None),
            model_preferences: None,
            system_prompt: None,
            temperature: None,
            max_tokens: 50,
            stop_sequences: None,
            metadata: None,
            tools: None,
            tool_choice: None,
        },
        extensions: Default::default(),
    });

    let result = handler
        .handle_request(
            request.clone(),
            RequestContext {
                peer: client.peer().clone(),
                ct: CancellationToken::new(),
                id: NumberOrString::Number(3),
                meta: Default::default(),
                extensions: Default::default(),
            },
        )
        .await;

    // This should result in an error
    assert!(result.is_err());

    client.cancel().await?;
    server_handle.await??;
    Ok(())
}

#[tokio::test]
async fn test_tool_choice_serialization() -> Result<()> {
    let auto = ToolChoice::auto();
    let json = serde_json::to_string(&auto)?;
    assert!(json.contains("auto"));
    let deserialized: ToolChoice = serde_json::from_str(&json)?;
    assert_eq!(auto, deserialized);

    let required = ToolChoice::required();
    let json = serde_json::to_string(&required)?;
    assert!(json.contains("required"));
    let deserialized: ToolChoice = serde_json::from_str(&json)?;
    assert_eq!(required, deserialized);

    let none = ToolChoice::none();
    let json = serde_json::to_string(&none)?;
    assert!(json.contains("none"));
    let deserialized: ToolChoice = serde_json::from_str(&json)?;
    assert_eq!(none, deserialized);

    Ok(())
}

#[tokio::test]
async fn test_sampling_with_tools() -> Result<()> {
    use std::sync::Arc;

    let tool = Tool::new(
        "get_weather",
        "Get the current weather for a location",
        Arc::new(
            serde_json::json!({
                "type": "object",
                "properties": {
                    "location": {
                        "type": "string",
                        "description": "The city and state, e.g. San Francisco, CA"
                    }
                },
                "required": ["location"]
            })
            .as_object()
            .unwrap()
            .clone(),
        ),
    );

    let params = CreateMessageRequestParams {
        meta: None,
        task: None,
        messages: vec![SamplingMessage::user_text(
            "What's the weather in San Francisco?",
        )],
        model_preferences: None,
        system_prompt: None,
        include_context: None,
        temperature: None,
        max_tokens: 100,
        stop_sequences: None,
        metadata: None,
        tools: Some(vec![tool]),
        tool_choice: Some(ToolChoice::auto()),
    };

    let json = serde_json::to_string(&params)?;
    let deserialized: CreateMessageRequestParams = serde_json::from_str(&json)?;

    assert!(deserialized.tools.is_some());
    assert_eq!(deserialized.tools.as_ref().unwrap().len(), 1);
    assert_eq!(deserialized.tools.as_ref().unwrap()[0].name, "get_weather");
    assert!(deserialized.tool_choice.is_some());

    Ok(())
}

#[tokio::test]
async fn test_tool_use_content_serialization() -> Result<()> {
    let tool_use = ToolUseContent::new(
        "call_123",
        "get_weather",
        serde_json::json!({
            "location": "San Francisco, CA"
        })
        .as_object()
        .unwrap()
        .clone(),
    );

    let json = serde_json::to_string(&tool_use)?;
    let deserialized: ToolUseContent = serde_json::from_str(&json)?;
    assert_eq!(tool_use, deserialized);
    assert_eq!(deserialized.id, "call_123");
    assert_eq!(deserialized.name, "get_weather");

    Ok(())
}

#[tokio::test]
async fn test_tool_result_content_serialization() -> Result<()> {
    let tool_result = ToolResultContent::new(
        "call_123",
        vec![Content::text(
            "The weather in San Francisco is 72°F and sunny.",
        )],
    );

    let json = serde_json::to_string(&tool_result)?;
    let deserialized: ToolResultContent = serde_json::from_str(&json)?;
    assert_eq!(tool_result, deserialized);
    assert_eq!(deserialized.tool_use_id, "call_123");
    assert!(!deserialized.content.is_empty());

    Ok(())
}

#[tokio::test]
async fn test_sampling_message_with_tool_use() -> Result<()> {
    let message = SamplingMessage::assistant_tool_use(
        "call_123",
        "get_weather",
        serde_json::json!({
            "location": "San Francisco, CA"
        })
        .as_object()
        .unwrap()
        .clone(),
    );

    let json = serde_json::to_string(&message)?;
    let deserialized: SamplingMessage = serde_json::from_str(&json)?;
    assert_eq!(message, deserialized);
    assert_eq!(deserialized.role, Role::Assistant);

    let tool_use = deserialized.content.first().unwrap().as_tool_use().unwrap();
    assert_eq!(tool_use.name, "get_weather");

    Ok(())
}

#[tokio::test]
async fn test_sampling_message_with_tool_result() -> Result<()> {
    let message =
        SamplingMessage::user_tool_result("call_123", vec![Content::text("72°F and sunny")]);

    let json = serde_json::to_string(&message)?;
    let deserialized: SamplingMessage = serde_json::from_str(&json)?;
    assert_eq!(message, deserialized);
    assert_eq!(deserialized.role, Role::User);

    let tool_result = deserialized
        .content
        .first()
        .unwrap()
        .as_tool_result()
        .unwrap();
    assert_eq!(tool_result.tool_use_id, "call_123");

    Ok(())
}

#[tokio::test]
async fn test_create_message_result_tool_use_stop_reason() -> Result<()> {
    let result = CreateMessageResult {
        message: SamplingMessage::assistant_tool_use(
            "call_123",
            "get_weather",
            serde_json::json!({
                "location": "San Francisco"
            })
            .as_object()
            .unwrap()
            .clone(),
        ),
        model: "test-model".to_string(),
        stop_reason: Some(CreateMessageResult::STOP_REASON_TOOL_USE.to_string()),
    };

    let json = serde_json::to_string(&result)?;
    let deserialized: CreateMessageResult = serde_json::from_str(&json)?;
    assert_eq!(result, deserialized);
    assert_eq!(deserialized.stop_reason, Some("toolUse".to_string()));

    Ok(())
}

#[tokio::test]
async fn test_sampling_capability() -> Result<()> {
    let cap = SamplingCapability {
        tools: Some(JsonObject::default()),
        context: None,
    };

    let json = serde_json::to_string(&cap)?;
    let deserialized: SamplingCapability = serde_json::from_str(&json)?;
    assert_eq!(cap, deserialized);
    assert!(deserialized.tools.is_some());
    assert!(deserialized.context.is_none());

    let client_cap = ClientCapabilities::builder()
        .enable_sampling()
        .enable_sampling_tools()
        .build();

    assert!(client_cap.sampling.is_some());
    assert!(client_cap.sampling.as_ref().unwrap().tools.is_some());

    Ok(())
}

#[tokio::test]
async fn test_backward_compat_sampling_message_deserialization() -> Result<()> {
    let old_format_json = r#"{
        "role": "user",
        "content": {
            "type": "text",
            "text": "Hello, world!"
        }
    }"#;

    let message: SamplingMessage = serde_json::from_str(old_format_json)?;
    assert_eq!(message.role, Role::User);
    let text = message.content.first().unwrap().as_text().unwrap();
    assert_eq!(text.text, "Hello, world!");

    Ok(())
}

#[tokio::test]
async fn test_backward_compat_sampling_message_with_image() -> Result<()> {
    let old_format_json = r#"{
        "role": "user",
        "content": {
            "type": "image",
            "data": "base64data",
            "mimeType": "image/png"
        }
    }"#;

    let message: SamplingMessage = serde_json::from_str(old_format_json)?;
    assert_eq!(message.role, Role::User);
    assert_eq!(message.content.len(), 1);

    Ok(())
}

#[tokio::test]
async fn test_backward_compat_sampling_capability_empty_object() -> Result<()> {
    let empty_json = "{}";
    let cap: SamplingCapability = serde_json::from_str(empty_json)?;
    assert!(cap.tools.is_none());
    assert!(cap.context.is_none());

    let client_cap_json = r#"{"sampling": {}}"#;
    let client_cap: ClientCapabilities = serde_json::from_str(client_cap_json)?;
    assert!(client_cap.sampling.is_some());

    Ok(())
}

#[tokio::test]
async fn test_content_to_sampling_message_content_conversion() -> Result<()> {
    use std::convert::TryInto;

    let content = Content::text("Hello");
    let sampling_content: SamplingMessageContent =
        content.try_into().map_err(|e: &str| anyhow::anyhow!(e))?;
    assert!(sampling_content.as_text().is_some());
    assert_eq!(sampling_content.as_text().unwrap().text, "Hello");

    let content = Content::image("base64data", "image/png");
    let sampling_content: SamplingMessageContent =
        content.try_into().map_err(|e: &str| anyhow::anyhow!(e))?;
    assert!(matches!(sampling_content, SamplingMessageContent::Image(_)));

    Ok(())
}

#[tokio::test]
async fn test_content_to_sampling_content_conversion() -> Result<()> {
    use std::convert::TryInto;

    let content = Content::text("Hello");
    let sampling_content: SamplingContent<SamplingMessageContent> =
        content.try_into().map_err(|e: &str| anyhow::anyhow!(e))?;
    assert_eq!(sampling_content.len(), 1);
    assert!(sampling_content.first().unwrap().as_text().is_some());

    Ok(())
}

#[tokio::test]
async fn test_content_conversion_unsupported_variants() {
    use std::convert::TryInto;

    use rmcp::model::ResourceContents;

    let resource_content = Content::resource(ResourceContents::TextResourceContents {
        uri: "file:///test.txt".to_string(),
        mime_type: Some("text/plain".to_string()),
        text: "test".to_string(),
        meta: None,
    });

    let result: Result<SamplingMessageContent, _> = resource_content.try_into();
    assert!(result.is_err());
    assert_eq!(
        result.unwrap_err(),
        "Resource content is not supported in sampling messages"
    );
}
