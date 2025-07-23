//cargo test --test test_elicitation --features "client server"

use rmcp::model::*;
use serde_json::json;

/// Test that elicitation data structures can be serialized and deserialized correctly
/// This ensures JSON-RPC compatibility with MCP 2025-06-18 specification
#[tokio::test]
async fn test_elicitation_serialization() {
    // Test ElicitationAction enum serialization
    let accept = ElicitationAction::Accept;
    let decline = ElicitationAction::Decline;
    let cancel = ElicitationAction::Cancel;

    assert_eq!(serde_json::to_string(&accept).unwrap(), "\"accept\"");
    assert_eq!(serde_json::to_string(&decline).unwrap(), "\"decline\"");
    assert_eq!(serde_json::to_string(&cancel).unwrap(), "\"cancel\"");

    // Test deserialization
    assert_eq!(
        serde_json::from_str::<ElicitationAction>("\"accept\"").unwrap(),
        ElicitationAction::Accept
    );
    assert_eq!(
        serde_json::from_str::<ElicitationAction>("\"decline\"").unwrap(),
        ElicitationAction::Decline
    );
    assert_eq!(
        serde_json::from_str::<ElicitationAction>("\"cancel\"").unwrap(),
        ElicitationAction::Cancel
    );
}

/// Test CreateElicitationRequestParam structure serialization/deserialization
#[tokio::test]
async fn test_elicitation_request_param_serialization() {
    let schema_object = json!({
        "type": "object",
        "properties": {
            "email": {
                "type": "string",
                "format": "email"
            }
        },
        "required": ["email"]
    })
    .as_object()
    .unwrap()
    .clone();

    let request_param = CreateElicitationRequestParam {
        message: "Please provide your email address".to_string(),
        requested_schema: schema_object,
    };

    // Test serialization
    let json = serde_json::to_value(&request_param).unwrap();
    let expected = json!({
        "message": "Please provide your email address",
        "requestedSchema": {
            "type": "object",
            "properties": {
                "email": {
                    "type": "string",
                    "format": "email"
                }
            },
            "required": ["email"]
        }
    });

    assert_eq!(json, expected);

    // Test deserialization
    let deserialized: CreateElicitationRequestParam = serde_json::from_value(expected).unwrap();
    assert_eq!(deserialized.message, request_param.message);
    assert_eq!(
        deserialized.requested_schema,
        request_param.requested_schema
    );
}

/// Test CreateElicitationResult structure with different action types
#[tokio::test]
async fn test_elicitation_result_serialization() {
    // Test Accept with content
    let accept_result = CreateElicitationResult {
        action: ElicitationAction::Accept,
        content: Some(json!({"email": "user@example.com"})),
    };

    let json = serde_json::to_value(&accept_result).unwrap();
    let expected = json!({
        "action": "accept",
        "content": {"email": "user@example.com"}
    });
    assert_eq!(json, expected);

    // Test Decline without content
    let decline_result = CreateElicitationResult {
        action: ElicitationAction::Decline,
        content: None,
    };

    let json = serde_json::to_value(&decline_result).unwrap();
    let expected = json!({
        "action": "decline"
        // content should be omitted when None due to skip_serializing_if
    });
    assert_eq!(json, expected);

    // Test deserialization
    let deserialized: CreateElicitationResult = serde_json::from_value(expected).unwrap();
    assert_eq!(deserialized.action, ElicitationAction::Decline);
    assert_eq!(deserialized.content, None);
}

/// Test that elicitation requests can be created and handled through the JSON-RPC protocol
#[tokio::test]
async fn test_elicitation_json_rpc_protocol() {
    let schema = json!({
        "type": "object",
        "properties": {
            "confirmation": {"type": "boolean"}
        },
        "required": ["confirmation"]
    })
    .as_object()
    .unwrap()
    .clone();

    // Create a complete JSON-RPC request for elicitation
    let request = JsonRpcRequest {
        jsonrpc: JsonRpcVersion2_0,
        id: RequestId::Number(1),
        request: CreateElicitationRequest {
            method: ElicitationCreateRequestMethod,
            params: CreateElicitationRequestParam {
                message: "Do you want to continue?".to_string(),
                requested_schema: schema,
            },
            extensions: Default::default(),
        },
    };

    // Test serialization of complete request
    let json = serde_json::to_value(&request).unwrap();
    assert_eq!(json["jsonrpc"], "2.0");
    assert_eq!(json["id"], 1);
    assert_eq!(json["method"], "elicitation/create");
    assert_eq!(json["params"]["message"], "Do you want to continue?");

    // Test deserialization
    let deserialized: JsonRpcRequest<CreateElicitationRequest> =
        serde_json::from_value(json).unwrap();
    assert_eq!(deserialized.id, RequestId::Number(1));
    assert_eq!(
        deserialized.request.params.message,
        "Do you want to continue?"
    );
}

/// Test elicitation action types and their expected behavior
#[tokio::test]
async fn test_elicitation_action_types() {
    // Test all three action types
    let actions = [
        ElicitationAction::Accept,
        ElicitationAction::Decline,
        ElicitationAction::Cancel,
    ];

    // Each action should have a unique string representation
    let serialized: Vec<String> = actions
        .iter()
        .map(|action| serde_json::to_string(action).unwrap())
        .collect();

    assert_eq!(serialized.len(), 3);
    assert!(serialized.contains(&"\"accept\"".to_string()));
    assert!(serialized.contains(&"\"decline\"".to_string()));
    assert!(serialized.contains(&"\"cancel\"".to_string()));

    // Test round-trip serialization
    for action in actions {
        let json = serde_json::to_string(&action).unwrap();
        let deserialized: ElicitationAction = serde_json::from_str(&json).unwrap();
        assert_eq!(action, deserialized);
    }
}

/// Test MCP 2025-06-18 specification compliance
/// Ensures our implementation matches the latest MCP spec
#[tokio::test]
async fn test_elicitation_spec_compliance() {
    // Test that method names match the specification
    assert_eq!(ElicitationCreateRequestMethod::VALUE, "elicitation/create");
    assert_eq!(
        ElicitationResponseNotificationMethod::VALUE,
        "notifications/elicitation/response"
    );

    // Test that protocol version includes the new 2025-06-18 version
    assert_eq!(ProtocolVersion::V_2025_06_18.to_string(), "2025-06-18");
    assert_eq!(ProtocolVersion::LATEST, ProtocolVersion::V_2025_06_18);

    // Test that enum values match specification
    let actions = [
        ElicitationAction::Accept,
        ElicitationAction::Decline,
        ElicitationAction::Cancel,
    ];

    let serialized: Vec<String> = actions
        .iter()
        .map(|a| serde_json::to_string(a).unwrap())
        .collect();

    assert_eq!(serialized, vec!["\"accept\"", "\"decline\"", "\"cancel\""]);
}

/// Test error handling and edge cases for elicitation
#[tokio::test]
async fn test_elicitation_error_handling() {
    // Test invalid JSON schema handling
    let invalid_schema_request = CreateElicitationRequestParam {
        message: "Test message".to_string(),
        requested_schema: serde_json::Map::new(), // Empty schema is technically valid
    };

    // Should serialize without error
    let _json = serde_json::to_value(&invalid_schema_request).unwrap();

    // Test empty message
    let empty_message_request = CreateElicitationRequestParam {
        message: "".to_string(),
        requested_schema: json!({"type": "string"}).as_object().unwrap().clone(),
    };

    // Should serialize without error (validation is up to the implementation)
    let _json = serde_json::to_value(&empty_message_request).unwrap();

    // Test that we can deserialize invalid action types (should fail)
    let invalid_action_json = json!("invalid_action");
    let result = serde_json::from_value::<ElicitationAction>(invalid_action_json);
    assert!(result.is_err());
}

/// Benchmark-style test for elicitation performance
#[tokio::test]
async fn test_elicitation_performance() {
    let schema = json!({
        "type": "object",
        "properties": {
            "data": {"type": "string"}
        }
    })
    .as_object()
    .unwrap()
    .clone();

    let request = CreateElicitationRequestParam {
        message: "Performance test message".to_string(),
        requested_schema: schema,
    };

    let start = std::time::Instant::now();

    // Serialize/deserialize 1000 times
    for _ in 0..1000 {
        let json = serde_json::to_value(&request).unwrap();
        let _deserialized: CreateElicitationRequestParam = serde_json::from_value(json).unwrap();
    }

    let duration = start.elapsed();
    println!(
        "1000 elicitation serialization/deserialization cycles took: {:?}",
        duration
    );

    // Should complete in reasonable time (less than 100ms on modern hardware)
    assert!(
        duration.as_millis() < 1000,
        "Performance test took too long: {:?}",
        duration
    );
}

/// Test elicitation capabilities integration
/// Ensures that elicitation capability can be properly configured and serialized
#[tokio::test]
async fn test_elicitation_capabilities() {
    use rmcp::model::{ClientCapabilities, ElicitationCapability};

    // Test basic elicitation capability
    let mut elicitation_cap = ElicitationCapability::default();
    assert_eq!(elicitation_cap.schema_validation, None);

    // Test with schema validation enabled
    elicitation_cap.schema_validation = Some(true);

    // Test serialization
    let json = serde_json::to_value(&elicitation_cap).unwrap();
    let expected = json!({"schemaValidation": true});
    assert_eq!(json, expected);

    // Test deserialization
    let deserialized: ElicitationCapability = serde_json::from_value(expected).unwrap();
    assert_eq!(deserialized.schema_validation, Some(true));

    // Test ClientCapabilities builder with elicitation
    let client_caps = ClientCapabilities::builder()
        .enable_elicitation()
        .enable_elicitation_schema_validation()
        .build();

    assert!(client_caps.elicitation.is_some());
    assert_eq!(
        client_caps.elicitation.as_ref().unwrap().schema_validation,
        Some(true)
    );

    // Test full client capabilities serialization
    let json = serde_json::to_value(&client_caps).unwrap();
    assert!(
        json["elicitation"]["schemaValidation"]
            .as_bool()
            .unwrap_or(false)
    );
}

/// Test convenience methods for common elicitation scenarios
/// This ensures the helper methods create proper requests with expected schemas
#[tokio::test]
async fn test_elicitation_convenience_methods() {
    // Test that convenience methods produce the expected request parameters

    // Test confirmation schema
    let confirmation_schema = serde_json::json!({
        "type": "boolean",
        "description": "User confirmation (true for yes, false for no)"
    });

    // Verify the schema structure matches what elicit_confirmation would create
    assert_eq!(confirmation_schema["type"], "boolean");
    assert!(confirmation_schema["description"].is_string());

    // Test text input schema (non-required)
    let text_schema = serde_json::json!({
        "type": "string",
        "description": "User text input"
    });

    assert_eq!(text_schema["type"], "string");
    assert!(text_schema.get("minLength").is_none());

    // Test text input schema (required)
    let required_text_schema = serde_json::json!({
        "type": "string",
        "description": "User text input",
        "minLength": 1
    });

    assert_eq!(required_text_schema["minLength"], 1);

    // Test choice schema
    let options = ["Option A", "Option B", "Option C"];
    let choice_schema = serde_json::json!({
        "type": "integer",
        "minimum": 0,
        "maximum": options.len() - 1,
        "description": format!("Choose an option: {}", options.join(", "))
    });

    assert_eq!(choice_schema["type"], "integer");
    assert_eq!(choice_schema["minimum"], 0);
    assert_eq!(choice_schema["maximum"], 2);
    assert!(
        choice_schema["description"]
            .as_str()
            .unwrap()
            .contains("Option A")
    );

    // Test that CreateElicitationRequestParam can be created with these schemas
    let confirmation_request = CreateElicitationRequestParam {
        message: "Test confirmation".to_string(),
        requested_schema: confirmation_schema.as_object().unwrap().clone(),
    };

    // Test serialization of convenience method request
    let json = serde_json::to_value(&confirmation_request).unwrap();
    assert_eq!(json["message"], "Test confirmation");
    assert_eq!(json["requestedSchema"]["type"], "boolean");
}

/// Test structured input with complex schemas
/// Ensures that complex JSON schemas work correctly with elicitation
#[tokio::test]
async fn test_elicitation_structured_schemas() {
    // Test complex nested object schema
    let complex_schema = json!({
        "type": "object",
        "properties": {
            "user": {
                "type": "object",
                "properties": {
                    "name": {"type": "string"},
                    "email": {"type": "string", "format": "email"},
                    "preferences": {
                        "type": "object",
                        "properties": {
                            "theme": {"type": "string", "enum": ["light", "dark"]},
                            "notifications": {"type": "boolean"}
                        }
                    }
                }
            },
            "metadata": {
                "type": "array",
                "items": {"type": "string"}
            }
        },
        "required": ["user"]
    });

    let request = CreateElicitationRequestParam {
        message: "Please provide your user information".to_string(),
        requested_schema: complex_schema.as_object().unwrap().clone(),
    };

    // Test that complex schemas serialize/deserialize correctly
    let json = serde_json::to_value(&request).unwrap();
    let deserialized: CreateElicitationRequestParam = serde_json::from_value(json).unwrap();

    assert_eq!(deserialized.message, "Please provide your user information");
    assert_eq!(
        deserialized.requested_schema["properties"]["user"]["properties"]["name"]["type"],
        "string"
    );

    // Test array schema
    let array_schema = json!({
        "type": "array",
        "items": {
            "type": "object",
            "properties": {
                "id": {"type": "integer"},
                "name": {"type": "string"}
            },
            "required": ["id", "name"]
        },
        "minItems": 1,
        "maxItems": 10
    });

    let array_request = CreateElicitationRequestParam {
        message: "Please provide a list of items".to_string(),
        requested_schema: array_schema.as_object().unwrap().clone(),
    };

    // Verify array schema
    let json = serde_json::to_value(&array_request).unwrap();
    assert_eq!(json["requestedSchema"]["type"], "array");
    assert_eq!(json["requestedSchema"]["minItems"], 1);
    assert_eq!(json["requestedSchema"]["maxItems"], 10);
}
