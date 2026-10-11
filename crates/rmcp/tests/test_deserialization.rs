use rmcp::model::{JsonRpcResponse, ServerJsonRpcMessage, ServerResult};
#[test]
fn test_tool_list_result() {
    let json = std::fs::read("tests/test_deserialization/tool_list_result.json").unwrap();
    let result: ServerJsonRpcMessage = serde_json::from_slice(&json).unwrap();
    println!("{result:#?}");

    assert!(matches!(
        result,
        ServerJsonRpcMessage::Response(JsonRpcResponse {
            result: ServerResult::ListToolsResult(_),
            ..
        })
    ));
}

/// Regression tests for `#[serde(untagged)]` deserialization of `ServerResult`.
///
/// `ServerResult` is an untagged enum, so serde tries each variant in declaration
/// order, with `CustomResult(Value)` acting as the catch-all. If variant ordering
/// changes, these tests will catch the regression.
mod untagged_server_result {
    use rmcp::model::{CallToolResult, JsonRpcResponse, ServerJsonRpcMessage, ServerResult};
    use serde_json::json;

    /// Helper: wrap a result value in a JSON-RPC response envelope.
    fn wrap_response(result: serde_json::Value) -> serde_json::Value {
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": result
        })
    }

    /// Parse a JSON-RPC response and return the inner `ServerResult`.
    fn parse_result(json: serde_json::Value) -> ServerResult {
        let msg: ServerJsonRpcMessage = serde_json::from_value(json).unwrap();
        match msg {
            ServerJsonRpcMessage::Response(JsonRpcResponse { result, .. }) => result,
            other => panic!("expected Response, got {other:?}"),
        }
    }

    #[test]
    fn initialize_result_deserializes_to_correct_variant() {
        let result = parse_result(wrap_response(json!({
            "protocolVersion": "2025-03-26",
            "capabilities": {},
            "serverInfo": {
                "name": "test-server",
                "version": "1.0.0"
            }
        })));
        assert!(
            matches!(result, ServerResult::InitializeResult(_)),
            "expected InitializeResult, got {result:?}"
        );
    }

    #[test]
    fn call_tool_result_deserializes_to_correct_variant() {
        let result = parse_result(wrap_response(json!({
            "content": [
                { "type": "text", "text": "hello" }
            ]
        })));
        assert!(
            matches!(result, ServerResult::CallToolResult(_)),
            "expected CallToolResult, got {result:?}"
        );
    }

    #[test]
    fn call_tool_result_with_null_structured_content_deserializes_to_correct_variant() {
        for payload in [
            json!({
                "resultType": "complete",
                "content": [],
                "structuredContent": null
            }),
            json!({
                "resultType": "complete",
                "structuredContent": null
            }),
            json!({ "structuredContent": null }),
        ] {
            let result = parse_result(wrap_response(payload.clone()));
            let ServerResult::CallToolResult(result) = result else {
                panic!("{payload} should deserialize as CallToolResult, got {result:?}");
            };
            assert_eq!(result.structured_content, Some(serde_json::Value::Null));
        }
    }

    #[test]
    fn null_structured_content_does_not_shadow_custom_result() {
        // Counting a present `structuredContent: null` as a known field must not
        // make CallToolResult swallow other objects carrying only null values.
        let result = parse_result(wrap_response(json!({
            "somethingElse": null
        })));
        assert!(
            matches!(result, ServerResult::CustomResult(_)),
            "expected CustomResult, got {result:?}"
        );
    }

    #[test]
    fn input_required_result_with_meta_deserializes_to_correct_variant() {
        let result = parse_result(wrap_response(json!({
            "resultType": "input_required",
            "inputRequests": {
                "username": {
                    "method": "elicitation/create",
                    "params": {
                        "message": "Please provide your username",
                        "requestedSchema": {
                            "type": "object",
                            "properties": {
                                "username": { "type": "string" }
                            },
                            "required": ["username"]
                        }
                    }
                }
            },
            "requestState": "opaque-state",
            "_meta": {
                "io.modelcontextprotocol/serverInfo": {
                    "name": "test-server",
                    "version": "1.0.0"
                }
            }
        })));

        let ServerResult::InputRequiredResult(result) = result else {
            panic!("expected InputRequiredResult, got {result:?}");
        };
        assert!(
            result.input_requests.is_some_and(|requests| {
                requests.len() == 1 && requests.contains_key("username")
            })
        );
        assert_eq!(result.request_state.as_deref(), Some("opaque-state"));
        assert_eq!(
            result
                .meta
                .as_ref()
                .and_then(|meta| meta.get("io.modelcontextprotocol/serverInfo")),
            Some(&json!({
                "name": "test-server",
                "version": "1.0.0"
            }))
        );
    }

    #[test]
    fn call_tool_result_rejects_input_required_discriminator() {
        assert!(
            serde_json::from_value::<CallToolResult>(json!({
                "resultType": "input_required",
                "requestState": "opaque-state",
                "_meta": {}
            }))
            .is_err()
        );
    }

    #[test]
    fn invalid_input_required_result_falls_through_to_custom_result() {
        let payload = json!({
            "resultType": "input_required",
            "_meta": {}
        });
        let result = parse_result(wrap_response(payload.clone()));

        let ServerResult::CustomResult(result) = result else {
            panic!("expected CustomResult, got {result:?}");
        };
        assert_eq!(result.0, payload);
    }

    #[test]
    fn empty_object_deserializes_to_empty_result() {
        let result = parse_result(wrap_response(json!({})));
        assert!(
            matches!(result, ServerResult::EmptyResult(_)),
            "expected EmptyResult, got {result:?}"
        );
    }

    #[test]
    fn unknown_shape_falls_through_to_custom_result() {
        // A value that doesn't match any known result type should land in
        // CustomResult.
        let result = parse_result(wrap_response(json!({
            "some_unknown_field": "some_value",
            "number": 42
        })));
        assert!(
            matches!(result, ServerResult::CustomResult(_)),
            "expected CustomResult, got {result:?}"
        );
    }

    #[test]
    fn result_type_bearing_objects_do_not_match_task_ack() {
        // TaskAckResult carries only `resultType` (+ optional `_meta`), so it
        // must not greedily swallow arbitrary results that happen to include
        // a `resultType` key inside the untagged ServerResult union.
        let result = parse_result(wrap_response(json!({
            "resultType": "weird-custom",
            "payload": { "a": 1 }
        })));
        assert!(
            matches!(result, ServerResult::CustomResult(_)),
            "expected CustomResult, got {result:?}"
        );

        let result = parse_result(wrap_response(json!({
            "resultType": "complete",
            "customField": 42
        })));
        assert!(
            matches!(result, ServerResult::CustomResult(_)),
            "expected CustomResult, got {result:?}"
        );

        // A bare complete ack (the actual tasks/update / tasks/cancel ack
        // shape) still parses as TaskAckResult.
        let result = parse_result(wrap_response(json!({ "resultType": "complete" })));
        assert!(
            matches!(result, ServerResult::TaskAckResult(_)),
            "expected TaskAckResult, got {result:?}"
        );
    }

    #[test]
    fn arbitrary_json_value_falls_through_to_custom_result() {
        // Any bare JSON value must fall through to CustomResult.
        for value in [json!(42), json!("hello"), json!(null), json!([1, 2, 3])] {
            let result = parse_result(wrap_response(value.clone()));
            assert!(
                matches!(result, ServerResult::CustomResult(_)),
                "value {value} should deserialize as CustomResult, got {result:?}"
            );
        }
    }

    #[test]
    fn round_trip_initialize_result_preserves_variant() {
        let json = json!({
            "protocolVersion": "2025-03-26",
            "capabilities": {},
            "serverInfo": { "name": "test", "version": "1.0" }
        });
        // Parse as ServerResult, serialize back, parse again — must stay InitializeResult.
        let result = parse_result(wrap_response(json.clone()));
        assert!(matches!(&result, ServerResult::InitializeResult(_)));
        let reserialized = serde_json::to_value(&result).unwrap();
        let result2 = parse_result(wrap_response(reserialized));
        assert!(matches!(result2, ServerResult::InitializeResult(_)));
    }

    #[test]
    fn round_trip_call_tool_result_preserves_variant() {
        let original =
            CallToolResult::success(vec![rmcp::model::ContentBlock::text("hello world")]);
        let json = serde_json::to_value(&original).unwrap();
        let result = parse_result(wrap_response(json));
        assert!(matches!(result, ServerResult::CallToolResult(_)));
    }
}

/// Regression tests for decimal float fields under serde_json's
/// `arbitrary_precision` feature, which rmcp's dev-dependencies enable.
///
/// With the feature, serde replays a decimal it buffered for an untagged enum
/// as serde_json's private number map. A plain `f32`/`f64` field rejected that
/// map, so each of these messages fell through to a `Custom*` variant. They are
/// decoded from text, which is where the map comes from.
mod arbitrary_precision {
    use rmcp::model::{
        ClientJsonRpcMessage, ClientNotification, ElicitRequestParams, JsonRpcMessage,
        JsonRpcNotification, JsonRpcRequest, JsonRpcResponse, PrimitiveSchemaDefinition,
        ServerJsonRpcMessage, ServerNotification, ServerRequest, ServerResult,
    };

    /// Decodes a server message the way a client does (`RxJsonRpcMessage<RoleClient>`).
    fn from_server(text: &str) -> ServerJsonRpcMessage {
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn feature_is_enabled() {
        // Only the feature keeps the number as written; without it this reads `0.6`.
        let value: serde_json::Value = serde_json::from_str("0.60").unwrap();
        assert_eq!(value.to_string(), "0.60");
    }

    #[test]
    fn call_tool_result_with_fractional_priority() {
        let message = from_server(
            r#"{"jsonrpc":"2.0","id":1,"result":{"content":[
                {"type":"text","text":"ok","annotations":{"priority":0.6}}]}}"#,
        );
        let JsonRpcMessage::Response(JsonRpcResponse {
            result: ServerResult::CallToolResult(result),
            ..
        }) = message
        else {
            panic!("expected CallToolResult, got {message:?}");
        };
        let annotations = result.content[0].as_text().unwrap().annotations.as_ref();
        assert_eq!(annotations.unwrap().priority, Some(0.6));
    }

    #[test]
    fn progress_notification_with_fractional_progress() {
        let text = r#"{"jsonrpc":"2.0","method":"notifications/progress",
            "params":{"progressToken":"t","progress":0.5,"total":2.5}}"#;

        let message = from_server(text);
        let JsonRpcMessage::Notification(JsonRpcNotification {
            notification: ServerNotification::ProgressNotification(notification),
            ..
        }) = message
        else {
            panic!("expected ProgressNotification, got {message:?}");
        };
        assert_eq!(notification.params.progress, 0.5);
        assert_eq!(notification.params.total, Some(2.5));

        let message: ClientJsonRpcMessage = serde_json::from_str(text).unwrap();
        assert!(
            matches!(
                message,
                JsonRpcMessage::Notification(JsonRpcNotification {
                    notification: ClientNotification::ProgressNotification(_),
                    ..
                })
            ),
            "expected ProgressNotification, got {message:?}"
        );
    }

    #[test]
    #[expect(deprecated, reason = "sampling is deprecated by SEP-2577")]
    fn create_message_request_with_fractional_parameters() {
        let message = from_server(
            r#"{"jsonrpc":"2.0","id":2,"method":"sampling/createMessage","params":{
                "messages":[],"maxTokens":10,"temperature":0.7,"modelPreferences":{
                "costPriority":0.2,"speedPriority":0.5,"intelligencePriority":0.9}}}"#,
        );
        let JsonRpcMessage::Request(JsonRpcRequest {
            request: ServerRequest::CreateMessageRequest(request),
            ..
        }) = message
        else {
            panic!("expected CreateMessageRequest, got {message:?}");
        };
        assert_eq!(request.params.temperature, Some(0.7));
        let preferences = request.params.model_preferences.unwrap();
        assert_eq!(preferences.cost_priority, Some(0.2));
        assert_eq!(preferences.speed_priority, Some(0.5));
        assert_eq!(preferences.intelligence_priority, Some(0.9));
    }

    #[test]
    fn elicit_request_with_fractional_number_schema() {
        let message = from_server(
            r#"{"jsonrpc":"2.0","id":3,"method":"elicitation/create","params":{
                "mode":"form","message":"How much?","requestedSchema":{"type":"object",
                "properties":{"amount":{"type":"number",
                "minimum":0.5,"maximum":9.5,"default":1.5}}}}}"#,
        );
        let JsonRpcMessage::Request(JsonRpcRequest {
            request: ServerRequest::ElicitRequest(request),
            ..
        }) = message
        else {
            panic!("expected ElicitRequest, got {message:?}");
        };
        let ElicitRequestParams::FormElicitationParams {
            requested_schema, ..
        } = request.params
        else {
            panic!("expected a form elicitation");
        };
        let Some(PrimitiveSchemaDefinition::Number(amount)) =
            requested_schema.properties.get("amount")
        else {
            panic!("expected a number schema");
        };
        assert_eq!(amount.minimum, Some(0.5));
        assert_eq!(amount.maximum, Some(9.5));
        assert_eq!(amount.default, Some(1.5));
    }
}
