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
/// order. `GetTaskPayloadResult` has a custom `Deserialize` impl that always fails
/// so it is skipped, and `CustomResult(Value)` acts as the catch-all. If variant
/// ordering changes or the custom impl is removed, these tests will catch the
/// regression.
mod untagged_server_result {
    use std::collections::BTreeMap;

    use rmcp::model::{
        CallToolRequest, CallToolRequestParams, CallToolResult, ClientRequest, DiscoverResult,
        ElicitationAction, InputRequiredResult, InputResponse, JsonRpcRequest, JsonRpcResponse,
        ProtocolVersion, ServerJsonRpcMessage, ServerResult,
    };
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
    fn call_tool_result_with_complete_result_type_deserializes_to_correct_variant() {
        let result = parse_result(wrap_response(json!({
            "resultType": "complete",
            "content": [
                { "type": "text", "text": "hello" }
            ],
            "isError": false
        })));
        assert!(
            matches!(result, ServerResult::CallToolResult(_)),
            "expected CallToolResult, got {result:?}"
        );
    }

    #[test]
    fn input_required_result_deserializes_to_custom_result() {
        let result = parse_result(wrap_response(json!({
            "resultType": "input_required",
            "inputRequests": {
                "confirm": {
                    "method": "elicitation/create",
                    "params": {
                        "mode": "form",
                        "message": "Delete 3 files?",
                        "requestedSchema": {
                            "type": "object",
                            "properties": {
                                "confirmed": { "type": "boolean" }
                            },
                            "required": ["confirmed"]
                        }
                    }
                }
            },
            "requestState": "opaque-state"
        })));
        let ServerResult::CustomResult(result) = result else {
            panic!("expected CustomResult, got {result:?}");
        };
        let input_required: InputRequiredResult = result.result_as().unwrap();
        assert_eq!(input_required.result_type.as_str(), "input_required");
        assert_eq!(
            input_required.request_state.as_deref(),
            Some("opaque-state")
        );
    }

    #[test]
    fn discover_result_deserializes_to_custom_result() {
        let result = parse_result(wrap_response(json!({
            "supportedVersions": ["2025-11-25", "2026-07-28"],
            "capabilities": {},
            "serverInfo": {
                "name": "test-server",
                "version": "1.0.0"
            },
            "instructions": "Use the tools carefully."
        })));
        let ServerResult::CustomResult(result) = result else {
            panic!("expected CustomResult, got {result:?}");
        };
        let discover: DiscoverResult = result.result_as().unwrap();
        assert_eq!(
            discover.supported_versions,
            vec![ProtocolVersion::V_2025_11_25, ProtocolVersion::V_2026_07_28]
        );
        assert_eq!(discover.server_info.name, "test-server");
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
        // CustomResult, NOT GetTaskPayloadResult.
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
    fn arbitrary_json_value_does_not_deserialize_as_get_task_payload_result() {
        // GetTaskPayloadResult wraps a bare Value, but its custom Deserialize
        // always fails so serde skips it during untagged resolution.
        // Any JSON value must fall through to CustomResult instead.
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

    #[test]
    fn discover_request_deserializes_to_custom_request() {
        let request: JsonRpcRequest<ClientRequest> = serde_json::from_value(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "server/discover",
            "params": {}
        }))
        .unwrap();

        let ClientRequest::CustomRequest(custom) = request.request else {
            panic!("expected CustomRequest, got {:?}", request.request);
        };
        assert_eq!(custom.method, "server/discover");
    }

    #[test]
    fn call_tool_request_round_trips_input_responses_and_request_state() {
        let mut input_responses = BTreeMap::new();
        input_responses.insert(
            "confirm".to_string(),
            InputResponse::ElicitResult(
                rmcp::model::ElicitResult::new(ElicitationAction::Accept)
                    .with_content(json!({ "confirmed": true })),
            ),
        );

        let request = CallToolRequest::new(
            CallToolRequestParams::new("delete_files")
                .with_arguments(serde_json::Map::from_iter([(
                    "path".to_string(),
                    json!("/tmp/example"),
                )]))
                .with_input_responses(input_responses)
                .with_request_state("opaque-state"),
        );

        let value = serde_json::to_value(&request).unwrap();
        assert_eq!(value["method"], "tools/call");
        assert_eq!(
            value["params"]["inputResponses"]["confirm"]["action"],
            "accept"
        );
        assert_eq!(value["params"]["requestState"], "opaque-state");

        let deserialized: CallToolRequest = serde_json::from_value(value).unwrap();
        assert_eq!(deserialized.params.name, "delete_files");
        assert_eq!(
            deserialized.params.request_state.as_deref(),
            Some("opaque-state")
        );
        assert!(
            deserialized
                .params
                .input_responses
                .as_ref()
                .is_some_and(|responses| responses.contains_key("confirm"))
        );
    }

    #[test]
    fn protocol_version_includes_2026_07_28_but_latest_remains_2025_11_25() {
        assert!(ProtocolVersion::KNOWN_VERSIONS.contains(&ProtocolVersion::V_2026_07_28));
        assert_eq!(ProtocolVersion::LATEST, ProtocolVersion::V_2025_11_25);
    }
}
