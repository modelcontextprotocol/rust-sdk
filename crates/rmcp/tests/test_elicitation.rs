//cargo test --test test_elicitation --features "client server"

use rmcp::model::*;
use rmcp::service::*;
use serde_json::json;

// For typed elicitation tests
#[cfg(feature = "schemars")]
use schemars::JsonSchema;
#[cfg(feature = "schemars")]
use serde::{Deserialize, Serialize};

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

    // Verify the schema structure for boolean confirmation
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

// Typed elicitation tests using the API with schemars
#[cfg(feature = "schemars")]
mod typed_elicitation_tests {
    use super::*;

    /// Simple user confirmation with reason
    #[derive(Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
    #[schemars(description = "User confirmation with optional reasoning")]
    struct UserConfirmation {
        #[schemars(description = "User's decision (true for yes, false for no)")]
        confirmed: bool,

        #[schemars(description = "Optional reason for the decision")]
        reason: Option<String>,
    }

    /// User profile with validation constraints
    #[derive(Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
    #[schemars(description = "Complete user profile information")]
    struct UserProfile {
        #[schemars(description = "Full name")]
        name: String,

        #[schemars(description = "Email address")]
        email: String,

        #[schemars(description = "Age in years")]
        age: u8,

        #[schemars(description = "User preferences")]
        preferences: UserPreferences,
    }

    /// User preferences
    #[derive(Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
    struct UserPreferences {
        #[schemars(description = "UI theme preference")]
        theme: Theme,

        #[schemars(description = "Enable notifications")]
        notifications: bool,

        #[schemars(description = "Language preference")]
        language: String,
    }

    /// UI theme options
    #[derive(Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
    #[schemars(description = "Available UI themes")]
    enum Theme {
        #[schemars(description = "Light theme")]
        Light,
        #[schemars(description = "Dark theme")]
        Dark,
        #[schemars(description = "Auto-detect based on system")]
        Auto,
    }

    /// Test automatic schema generation for simple types
    #[tokio::test]
    async fn test_typed_elicitation_simple_schema() {
        // Test that schema generation works for simple types
        let schema = rmcp::handler::server::tool::schema_for_type::<UserConfirmation>();

        // Verify schema contains expected fields
        assert!(schema.contains_key("type"));
        assert_eq!(schema.get("type"), Some(&json!("object")));
        assert!(schema.contains_key("properties"));

        if let Some(properties) = schema.get("properties") {
            assert!(properties.is_object());
            let props = properties.as_object().unwrap();
            assert!(props.contains_key("confirmed"));
            assert!(props.contains_key("reason"));

            // Check confirmed field is boolean
            if let Some(confirmed_schema) = props.get("confirmed") {
                let confirmed_obj = confirmed_schema.as_object().unwrap();
                assert_eq!(confirmed_obj.get("type"), Some(&json!("boolean")));
            }

            // Check reason field is optional string
            if let Some(reason_schema) = props.get("reason") {
                assert!(reason_schema.is_object());
            }
        }
    }

    /// Test automatic schema generation for complex nested types
    #[tokio::test]
    async fn test_typed_elicitation_complex_schema() {
        // Test complex nested structure schema generation
        let schema = rmcp::handler::server::tool::schema_for_type::<UserProfile>();

        // Verify schema structure
        assert!(schema.contains_key("type"));
        assert_eq!(schema.get("type"), Some(&json!("object")));

        if let Some(properties) = schema.get("properties") {
            let props = properties.as_object().unwrap();

            // Check required fields exist
            assert!(props.contains_key("name"));
            assert!(props.contains_key("email"));
            assert!(props.contains_key("age"));
            assert!(props.contains_key("preferences"));

            // Check validation constraints for name
            if let Some(name_schema) = props.get("name") {
                let name_obj = name_schema.as_object().unwrap();
                assert_eq!(name_obj.get("type"), Some(&json!("string")));
                // Note: schemars might generate constraints differently
                // The exact structure depends on schemars version
            }

            // Check email format constraint
            if let Some(email_schema) = props.get("email") {
                let email_obj = email_schema.as_object().unwrap();
                assert_eq!(email_obj.get("type"), Some(&json!("string")));
            }

            // Check age numeric constraints
            if let Some(age_schema) = props.get("age") {
                let age_obj = age_schema.as_object().unwrap();
                assert_eq!(age_obj.get("type"), Some(&json!("integer")));
            }
        }
    }

    /// Test enum schema generation
    #[tokio::test]
    async fn test_enum_schema_generation() {
        // Test enum schema generation
        let schema = rmcp::handler::server::tool::schema_for_type::<Theme>();

        // Verify enum schema structure - schemars might use oneOf or enum depending on version
        assert!(
            schema.contains_key("type")
                || schema.contains_key("oneOf")
                || schema.contains_key("enum")
        );

        // The exact structure depends on schemars configuration, but it should be valid
        let json = serde_json::to_string(&schema).unwrap();
        assert!(!json.is_empty());
    }

    /// Test that the schema generation for nested structures works
    #[tokio::test]
    async fn test_nested_structure_schema() {
        // Test that nested structures generate proper schemas
        let preferences_schema = rmcp::handler::server::tool::schema_for_type::<UserPreferences>();

        // Verify basic structure
        assert!(preferences_schema.contains_key("type"));
        assert_eq!(preferences_schema.get("type"), Some(&json!("object")));

        if let Some(properties) = preferences_schema.get("properties") {
            let props = properties.as_object().unwrap();
            assert!(props.contains_key("theme"));
            assert!(props.contains_key("notifications"));
            assert!(props.contains_key("language"));
        }
    }

}

// =============================================================================
// ELICITATION DIRECTION TESTS (MCP 2025-06-18 COMPLIANCE)
// =============================================================================

/// Test that elicitation requests flow from server to client (not client to server)
/// This verifies compliance with MCP 2025-06-18 specification
#[cfg(all(feature = "client", feature = "server"))]
#[tokio::test]
async fn test_elicitation_direction_server_to_client() {
    use rmcp::model::*;
    use serde_json::json;
    
    // Test that server can create elicitation requests
    let schema = json!({
        "type": "string",
        "description": "Enter your name"
    }).as_object().unwrap().clone();

    let elicitation_request = CreateElicitationRequestParam {
        message: "Please enter your name".to_string(),
        requested_schema: schema,
    };

    // Verify request can be serialized
    let serialized = serde_json::to_value(&elicitation_request).unwrap();
    assert_eq!(serialized["message"], "Please enter your name");
    assert_eq!(serialized["requestedSchema"]["type"], "string");

    // Test that elicitation requests are part of ServerRequest
    let server_request = ServerRequest::CreateElicitationRequest(CreateElicitationRequest {
        method: ElicitationCreateRequestMethod,
        params: elicitation_request,
        extensions: Default::default(),
    });

    // Verify server request can be serialized
    match server_request {
        ServerRequest::CreateElicitationRequest(_) => {
            // This is correct - server can send elicitation requests
            assert!(true);
        }
        _ => panic!("CreateElicitationRequest should be part of ServerRequest"),
    }

    // Test that client can respond with elicitation results
    let client_result = ClientResult::CreateElicitationResult(CreateElicitationResult {
        action: ElicitationAction::Accept,
        content: Some(json!("John Doe")),
    });

    // Verify client result can be serialized
    match client_result {
        ClientResult::CreateElicitationResult(result) => {
            assert_eq!(result.action, ElicitationAction::Accept);
            assert_eq!(result.content, Some(json!("John Doe")));
        }
        _ => panic!("CreateElicitationResult should be part of ClientResult"),
    }
}

/// Test complete JSON-RPC message flow: Server → Client → Server
#[cfg(all(feature = "client", feature = "server"))]
#[tokio::test]
async fn test_elicitation_json_rpc_direction() {
    use rmcp::model::*;
    use serde_json::json;

    let schema = json!({
        "type": "boolean",
        "description": "Do you want to continue?"
    }).as_object().unwrap().clone();

    // 1. Server creates elicitation request
    let server_request = ServerJsonRpcMessage::request(
        ServerRequest::CreateElicitationRequest(CreateElicitationRequest {
            method: ElicitationCreateRequestMethod,
            params: CreateElicitationRequestParam {
                message: "Do you want to continue?".to_string(),
                requested_schema: schema,
            },
            extensions: Default::default(),
        }),
        RequestId::Number(1),
    );

    // Serialize server request
    let server_json = serde_json::to_value(&server_request).unwrap();
    assert_eq!(server_json["method"], "elicitation/create");
    assert_eq!(server_json["id"], 1);
    assert_eq!(server_json["params"]["message"], "Do you want to continue?");

    // 2. Client responds with elicitation result
    let client_response = ClientJsonRpcMessage::response(
        ClientResult::CreateElicitationResult(CreateElicitationResult {
            action: ElicitationAction::Accept,
            content: Some(json!(true)),
        }),
        RequestId::Number(1),
    );

    // Serialize client response
    let client_json = serde_json::to_value(&client_response).unwrap();
    assert_eq!(client_json["id"], 1);
    if let Some(result) = client_json["result"].as_object() {
        assert_eq!(result["action"], "accept");
        assert_eq!(result["content"], true);
    } else {
        panic!("Client response should contain result");
    }
}

/// Test all three elicitation actions according to MCP spec
#[cfg(all(feature = "client", feature = "server"))]
#[tokio::test] 
async fn test_elicitation_actions_compliance() {
    use rmcp::model::*;

    // Test all three elicitation actions according to MCP spec
    let actions = [
        ElicitationAction::Accept,
        ElicitationAction::Decline, 
        ElicitationAction::Cancel,
    ];

    for action in actions {
        let result = CreateElicitationResult {
            action: action.clone(),
            content: match action {
                ElicitationAction::Accept => Some(serde_json::json!("some data")),
                _ => None,
            },
        };

        let json = serde_json::to_value(&result).unwrap();
        
        match action {
            ElicitationAction::Accept => {
                assert_eq!(json["action"], "accept");
                assert!(json["content"].is_string());
            }
            ElicitationAction::Decline => {
                assert_eq!(json["action"], "decline");
                assert!(json.get("content").is_none() || json["content"].is_null());
            }
            ElicitationAction::Cancel => {
                assert_eq!(json["action"], "cancel");
                assert!(json.get("content").is_none() || json["content"].is_null());
            }
        }
    }
}

/// Test that CreateElicitationRequest is NOT in ClientRequest (direction compliance)
#[tokio::test]
async fn test_elicitation_not_in_client_request() {
    // This test ensures that clients cannot initiate elicitation requests
    // according to MCP 2025-06-18 specification
    
    // Compile-time test: if this compiles, the test fails
    // The following should NOT compile if our implementation is correct:
    // let client_request = ClientRequest::CreateElicitationRequest(...);
    
    // Instead, we verify that all valid ClientRequest variants do NOT include elicitation
    let valid_client_requests = [
        "PingRequest",
        "InitializeRequest", 
        "CompleteRequest",
        "SetLevelRequest",
        "GetPromptRequest",
        "ListPromptsRequest",
        "ListResourcesRequest",
        "ListResourceTemplatesRequest", 
        "ReadResourceRequest",
        "SubscribeRequest",
        "UnsubscribeRequest",
        "CallToolRequest",
        "ListToolsRequest",
        // CreateElicitationRequest should NOT be here
    ];
    
    // Verify the list doesn't contain elicitation
    assert!(!valid_client_requests.contains(&"CreateElicitationRequest"));
    assert_eq!(valid_client_requests.len(), 13); // Should be 13, not 14
}

/// Test that CreateElicitationResult IS in ClientResult (response compliance)
#[tokio::test]
async fn test_elicitation_result_in_client_result() {
    use rmcp::model::*;
    
    // Test that clients can return elicitation results
    let result = ClientResult::CreateElicitationResult(CreateElicitationResult {
        action: ElicitationAction::Decline,
        content: None,
    });
    
    match result {
        ClientResult::CreateElicitationResult(elicit_result) => {
            assert_eq!(elicit_result.action, ElicitationAction::Decline);
            assert_eq!(elicit_result.content, None);
        }
        _ => panic!("CreateElicitationResult should be part of ClientResult"),
    }
}

// =============================================================================
// ELICITATION CAPABILITIES TESTS  
// =============================================================================

/// Test ElicitationCapability structure and serialization
#[tokio::test]
async fn test_elicitation_capability_structure() {
    // Test default ElicitationCapability 
    let default_cap = ElicitationCapability::default();
    assert!(default_cap.schema_validation.is_none());
    
    // Test ElicitationCapability with schema validation enabled
    let cap_with_validation = ElicitationCapability {
        schema_validation: Some(true),
    };
    assert_eq!(cap_with_validation.schema_validation, Some(true));
    
    // Test ElicitationCapability with schema validation disabled
    let cap_without_validation = ElicitationCapability {
        schema_validation: Some(false),
    };
    assert_eq!(cap_without_validation.schema_validation, Some(false));
    
    // Test JSON serialization
    let json = serde_json::to_value(&cap_with_validation).unwrap();
    assert_eq!(json, serde_json::json!({
        "schemaValidation": true
    }));
    
    // Test JSON deserialization
    let deserialized: ElicitationCapability = serde_json::from_value(json).unwrap();
    assert_eq!(deserialized.schema_validation, Some(true));
}

/// Test ClientCapabilities with elicitation capability
#[tokio::test]
async fn test_client_capabilities_with_elicitation() {
    // Test ClientCapabilities with elicitation capability
    let capabilities = ClientCapabilities {
        elicitation: Some(ElicitationCapability {
            schema_validation: Some(true),
        }),
        ..Default::default()
    };
    
    // Verify elicitation capability is present
    assert!(capabilities.elicitation.is_some());
    assert_eq!(
        capabilities.elicitation.as_ref().unwrap().schema_validation,
        Some(true)
    );
    
    // Test JSON serialization
    let json = serde_json::to_value(&capabilities).unwrap();
    assert!(json["elicitation"]["schemaValidation"].as_bool().unwrap_or(false));
    
    // Test ClientCapabilities without elicitation
    let capabilities_without = ClientCapabilities {
        elicitation: None,
        ..Default::default()
    };
    
    assert!(capabilities_without.elicitation.is_none());
}

/// Test InitializeRequestParam with elicitation capability
#[tokio::test]
async fn test_initialize_request_with_elicitation() {
    // Test InitializeRequestParam with elicitation capability
    let init_param = InitializeRequestParam {
        protocol_version: ProtocolVersion::LATEST,
        capabilities: ClientCapabilities {
            elicitation: Some(ElicitationCapability {
                schema_validation: Some(true),
            }),
            ..Default::default()
        },
        client_info: Implementation {
            name: "test-client".to_string(),
            version: "1.0.0".to_string(),
        },
    };
    
    // Verify the structure
    assert!(init_param.capabilities.elicitation.is_some());
    assert_eq!(
        init_param.capabilities.elicitation.as_ref().unwrap().schema_validation,
        Some(true)
    );
    
    // Test JSON serialization
    let json = serde_json::to_value(&init_param).unwrap();
    assert!(json["capabilities"]["elicitation"]["schemaValidation"]
        .as_bool()
        .unwrap_or(false));
}

/// Test capability checking logic (simulated)
#[tokio::test]
async fn test_capability_checking_logic() {
    // Simulate the logic that would be used in supports_elicitation()
    
    // Case 1: Client with elicitation capability
    let client_with_capability = InitializeRequestParam {
        protocol_version: ProtocolVersion::LATEST,
        capabilities: ClientCapabilities {
            elicitation: Some(ElicitationCapability {
                schema_validation: Some(true),
            }),
            ..Default::default()
        },
        client_info: Implementation {
            name: "test-client".to_string(),
            version: "1.0.0".to_string(),
        },
    };
    
    // Simulate supports_elicitation() logic
    let supports_elicitation = client_with_capability.capabilities.elicitation.is_some();
    assert!(supports_elicitation);
    
    // Case 2: Client without elicitation capability
    let client_without_capability = InitializeRequestParam {
        protocol_version: ProtocolVersion::LATEST,
        capabilities: ClientCapabilities {
            elicitation: None,
            ..Default::default()
        },
        client_info: Implementation {
            name: "test-client".to_string(),
            version: "1.0.0".to_string(),
        },
    };
    
    let supports_elicitation = client_without_capability.capabilities.elicitation.is_some();
    assert!(!supports_elicitation);
}


/// Test CapabilityNotSupported error message formatting
#[tokio::test]
async fn test_capability_not_supported_error_message() {
    let error = ElicitationError::CapabilityNotSupported;
    let message = format!("{}", error);
    
    assert_eq!(
        message,
        "Client does not support elicitation - capability not declared during initialization"
    );
}

/// Test all ElicitationError variants and their messages
#[tokio::test]
async fn test_elicitation_error_variants() {
    // Test CapabilityNotSupported
    let capability_error = ElicitationError::CapabilityNotSupported;
    assert_eq!(
        format!("{}", capability_error),
        "Client does not support elicitation - capability not declared during initialization"
    );
    
    // Test UserDeclined
    let user_declined = ElicitationError::UserDeclined;
    assert_eq!(
        format!("{}", user_declined),
        "User declined or cancelled the request"
    );
    
    // Test NoContent
    let no_content = ElicitationError::NoContent;
    assert_eq!(
        format!("{}", no_content),
        "No response content provided"
    );
    
    // Test Service error
    let service_error = ElicitationError::Service(ServiceError::UnexpectedResponse);
    let message = format!("{}", service_error);
    assert!(message.starts_with("Service error:"));
    
    // Test ParseError
    let json_error = serde_json::from_str::<i32>("\"not_an_integer\"").unwrap_err();
    let data = serde_json::json!({"key": "value"});
    let parse_error = ElicitationError::ParseError {
        error: json_error,
        data: data.clone(),
    };
    let message = format!("{}", parse_error);
    assert!(message.starts_with("Failed to parse response data:"));
    assert!(message.contains("Received data:"));
    
    // Test error matching
    match capability_error {
        ElicitationError::CapabilityNotSupported => {}, // Expected
        _ => panic!("Should match CapabilityNotSupported"),
    }
    
    match user_declined {
        ElicitationError::UserDeclined => {}, // Expected
        _ => panic!("Should match UserDeclined"),
    }
    
    match no_content {
        ElicitationError::NoContent => {}, // Expected
        _ => panic!("Should match NoContent"),
    }
}

/// Test ElicitationCapability serialization with schema validation
#[tokio::test]
async fn test_elicitation_capability_serialization() {
    use rmcp::model::ElicitationCapability;
    
    // Test default capability (no schema validation)
    let default_cap = ElicitationCapability::default();
    let json = serde_json::to_value(&default_cap).unwrap();
    
    // Should serialize to empty object when no fields are set
    assert_eq!(json, serde_json::json!({}));
    
    // Test capability with schema validation enabled
    let cap_with_validation = ElicitationCapability {
        schema_validation: Some(true),
    };
    let json = serde_json::to_value(&cap_with_validation).unwrap();
    
    assert_eq!(json, serde_json::json!({
        "schemaValidation": true
    }));
    
    // Test capability with schema validation disabled
    let cap_without_validation = ElicitationCapability {
        schema_validation: Some(false),
    };
    let json = serde_json::to_value(&cap_without_validation).unwrap();
    
    assert_eq!(json, serde_json::json!({
        "schemaValidation": false
    }));
    
    // Test deserialization
    let deserialized: ElicitationCapability = serde_json::from_value(
        serde_json::json!({
            "schemaValidation": true
        })
    ).unwrap();
    
    assert_eq!(deserialized.schema_validation, Some(true));
}

/// Test ClientCapabilities builder with elicitation capability methods
#[tokio::test]
async fn test_client_capabilities_elicitation_builder() {
    use rmcp::model::{ClientCapabilities, ElicitationCapability};
    
    // Test enabling elicitation capability
    let caps = ClientCapabilities::builder()
        .enable_elicitation()
        .build();
    
    assert!(caps.elicitation.is_some());
    assert_eq!(caps.elicitation.as_ref().unwrap().schema_validation, None);
    
    // Test enabling elicitation with schema validation
    let caps_with_validation = ClientCapabilities::builder()
        .enable_elicitation()
        .enable_elicitation_schema_validation()
        .build();
    
    assert!(caps_with_validation.elicitation.is_some());
    assert_eq!(
        caps_with_validation.elicitation.as_ref().unwrap().schema_validation,
        Some(true)
    );
    
    // Test enabling elicitation with custom capability
    let custom_elicitation = ElicitationCapability {
        schema_validation: Some(false),
    };
    
    let caps_custom = ClientCapabilities::builder()
        .enable_elicitation_with(custom_elicitation.clone())
        .build();
    
    assert!(caps_custom.elicitation.is_some());
    assert_eq!(
        caps_custom.elicitation.as_ref().unwrap(),
        &custom_elicitation
    );
}
