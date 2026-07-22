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
