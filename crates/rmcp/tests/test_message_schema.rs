mod tests {
    use rmcp::model::{ClientJsonRpcMessage, ServerJsonRpcMessage};
    use schemars::generate::SchemaSettings;

    fn compare_schemas(name: &str, actual: &str, expected_file: &str) {
        let expected = match std::fs::read_to_string(expected_file) {
            Ok(content) => content,
            Err(e) => {
                panic!(
                    "Failed to read expected schema file {}: {}",
                    expected_file, e
                );
            }
        };

        let actual_json: serde_json::Value =
            serde_json::from_str(actual).expect("Failed to parse actual schema as JSON");
        let expected_json: serde_json::Value =
            serde_json::from_str(&expected).expect("Failed to parse expected schema as JSON");

        if actual_json == expected_json {
            println!("{} schema matches expected", name);
            return;
        }

        // Write current schema to file for comparison
        let current_file = expected_file.replace(".json", "_current.json");
        std::fs::write(&current_file, actual).expect("Failed to write current schema");

        println!("{} schema differs from expected", name);
        println!("Expected: {}", expected_file);
        println!("Current: {}", current_file);
        println!(
            "Run 'diff {} {}' to see differences",
            expected_file, current_file
        );

        // UPDATE_SCHEMA=1 cargo test -p rmcp --test test_message_schema --features="server client schemars"
        if std::env::var("UPDATE_SCHEMA").is_ok() {
            println!("UPDATE_SCHEMA is set, updating expected file");
            std::fs::write(expected_file, actual).expect("Failed to update expected schema file");
            println!("Updated {}", expected_file);
        } else {
            println!("Set UPDATE_SCHEMA=1 to auto-update expected schemas");
            panic!("Schema validation failed");
        }
    }

    #[test]
    fn test_client_json_rpc_message_schema() {
        let settings = SchemaSettings::draft07();
        let schema = settings
            .into_generator()
            .into_root_schema_for::<ClientJsonRpcMessage>();
        let schema_str = serde_json::to_string_pretty(&schema).expect("Failed to serialize schema");

        compare_schemas(
            "ClientJsonRpcMessage",
            &schema_str,
            "tests/test_message_schema/client_json_rpc_message_schema.json",
        );
    }

    /// The three metadata definitions must expose the MCP 2026-07-28
    /// vocabulary: `MetaObject` is an open map, `RequestMetaObject` reserves
    /// `progressToken` plus the SEP-2575 keys, and `NotificationMetaObject`
    /// reserves `io.modelcontextprotocol/subscriptionId`. Version-specific
    /// required keys stay optional because rmcp generates one schema shared
    /// by every supported protocol version; current-version validation is
    /// a runtime concern (`RequestMetaObject::missing_required_keys`).
    #[test]
    fn test_metadata_definitions_match_2026_07_28_schema() {
        let settings = SchemaSettings::draft07();
        let schema = settings
            .into_generator()
            .into_root_schema_for::<ClientJsonRpcMessage>();
        let schema = serde_json::to_value(&schema).expect("Failed to serialize schema");
        let definitions = &schema["definitions"];

        assert_eq!(
            definitions["MetaObject"],
            serde_json::json!({
                "description": "See [MCP general fields](https://modelcontextprotocol.io/specification/2026-07-28/basic#general-fields) for notes on _meta usage.",
                "type": "object",
                "additionalProperties": true,
            })
        );

        assert_eq!(
            definitions["RequestMetaObject"],
            serde_json::json!({
                "description": "Metadata reserved by MCP on requests. Extension keys are also allowed.",
                "type": "object",
                "properties": {
                    "progressToken": { "$ref": "#/definitions/ProgressToken" },
                    "io.modelcontextprotocol/protocolVersion": { "type": "string" },
                    "io.modelcontextprotocol/clientInfo": { "$ref": "#/definitions/Implementation" },
                    "io.modelcontextprotocol/clientCapabilities": { "$ref": "#/definitions/ClientCapabilities" },
                    "io.modelcontextprotocol/logLevel": { "$ref": "#/definitions/LoggingLevel" },
                },
                "additionalProperties": true,
            })
        );

        assert_eq!(
            definitions["NotificationMetaObject"],
            serde_json::json!({
                "description": "Metadata reserved by MCP on notifications. Extension keys are also allowed.",
                "type": "object",
                "properties": {
                    "io.modelcontextprotocol/subscriptionId": { "$ref": "#/definitions/NumberOrString" },
                },
                "additionalProperties": true,
            })
        );
    }

    #[test]
    fn test_server_json_rpc_message_schema() {
        let settings = SchemaSettings::draft07();
        let schema = settings
            .into_generator()
            .into_root_schema_for::<ServerJsonRpcMessage>();
        let schema_value =
            serde_json::to_value(&schema).expect("Failed to serialize server schema");
        let discover_result = &schema_value["definitions"]["DiscoverResult"];
        assert!(
            discover_result["properties"].get("serverInfo").is_none(),
            "DiscoverResult serverInfo belongs in namespaced _meta"
        );
        let schema_str = serde_json::to_string_pretty(&schema).expect("Failed to serialize schema");

        compare_schemas(
            "ServerJsonRpcMessage",
            &schema_str,
            "tests/test_message_schema/server_json_rpc_message_schema.json",
        );
    }
}
