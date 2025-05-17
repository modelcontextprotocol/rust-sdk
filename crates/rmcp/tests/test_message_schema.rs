mod tests {
    use rmcp::model::{ClientJsonRpcMessage, ServerJsonRpcMessage};
    use schemars::schema_for;

    #[test]
    fn test_client_json_rpc_message_schema() {
        let schema = schema_for!(ClientJsonRpcMessage);
        let schema_str = serde_json::to_string_pretty(&schema).unwrap();
        let expected = std::fs::read_to_string(
            "tests/test_message_schema/client_json_rpc_message_schema.json",
        )
        .unwrap();

        // Parse both strings to JSON values for more robust comparison
        let schema_json: serde_json::Value = serde_json::from_str(&schema_str).unwrap();
        let expected_json: serde_json::Value = serde_json::from_str(&expected).unwrap();
        assert_eq!(
            schema_json, expected_json,
            "Schema generation for ClientJsonRpcMessage should match expected output"
        );
    }

    #[test]
    fn test_server_json_rpc_message_schema() {
        let schema = schema_for!(ServerJsonRpcMessage);
        let schema_str = serde_json::to_string_pretty(&schema).unwrap();
        let expected = std::fs::read_to_string(
            "tests/test_message_schema/server_json_rpc_message_schema.json",
        )
        .unwrap();

        // Parse both strings to JSON values for more robust comparison
        let schema_json: serde_json::Value = serde_json::from_str(&schema_str).unwrap();
        let expected_json: serde_json::Value = serde_json::from_str(&expected).unwrap();
        assert_eq!(
            schema_json, expected_json,
            "Schema generation for ServerJsonRpcMessage should match expected output"
        );
    }
}
