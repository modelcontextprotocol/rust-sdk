use rmcp::model::{
    ClientCapabilities, ClientJsonRpcMessage, ClientRequest, DiscoverResult, ErrorCode, ErrorData,
    JsonRpcRequest, JsonRpcResponse, ProtocolVersion, ServerJsonRpcMessage, ServerResult,
};
use serde_json::json;

#[test]
fn discover_request_deserializes_with_request_meta() {
    let message: ClientJsonRpcMessage = serde_json::from_value(json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "server/discover",
        "params": {
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                "io.modelcontextprotocol/clientInfo": {
                    "name": "test-client",
                    "version": "1.0.0"
                },
                "io.modelcontextprotocol/clientCapabilities": {}
            }
        }
    }))
    .expect("discover request should deserialize");

    let ClientJsonRpcMessage::Request(JsonRpcRequest { request, .. }) = message else {
        panic!("expected request");
    };
    let ClientRequest::DiscoverRequest(request) = request else {
        panic!("expected discover request");
    };

    assert_eq!(
        request
            .extensions
            .get::<rmcp::model::RequestMetaObject>()
            .and_then(|meta| meta.protocol_version()),
        Some(ProtocolVersion::V_2026_07_28)
    );
}

#[test]
fn discover_result_deserializes_to_typed_variant() {
    let message: ServerJsonRpcMessage = serde_json::from_value(json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": {
            "resultType": "complete",
            "supportedVersions": ["2025-11-25", "2026-07-28"],
            "capabilities": { "tools": {} },
            "serverInfo": {
                "name": "test-server",
                "version": "1.0.0"
            },
            "ttlMs": 0,
            "cacheScope": "private"
        }
    }))
    .expect("discover result should deserialize");

    let ServerJsonRpcMessage::Response(JsonRpcResponse { result, .. }) = message else {
        panic!("expected response");
    };
    let ServerResult::DiscoverResult(DiscoverResult {
        supported_versions, ..
    }) = result
    else {
        panic!("expected discover result");
    };

    assert_eq!(
        supported_versions,
        vec![ProtocolVersion::V_2025_11_25, ProtocolVersion::V_2026_07_28]
    );
}

#[test]
fn discover_result_accepts_server_info_in_namespaced_metadata() {
    let message: ServerJsonRpcMessage = serde_json::from_value(json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": {
            "resultType": "complete",
            "supportedVersions": ["2026-07-28"],
            "capabilities": {},
            "ttlMs": 0,
            "cacheScope": "private",
            "_meta": {
                "io.modelcontextprotocol/serverInfo": {
                    "name": "conformance-mock-server",
                    "version": "1.0.0"
                },
                "unrelated": { "preserved": true }
            }
        }
    }))
    .expect("discovery response with namespaced server info should deserialize");

    let ServerJsonRpcMessage::Response(JsonRpcResponse { result, .. }) = message else {
        panic!("expected response");
    };
    let ServerResult::DiscoverResult(result) = result else {
        panic!("expected discovery response, not a tool-call result");
    };

    let server_info = result.server_info().expect("server info should be present");
    assert_eq!(server_info.name, "conformance-mock-server");
    assert_eq!(server_info.version, "1.0.0");

    let metadata = result.meta.expect("discovery metadata should be preserved");
    assert_eq!(
        metadata.0.get("io.modelcontextprotocol/serverInfo"),
        Some(&json!({
            "name": "conformance-mock-server",
            "version": "1.0.0"
        }))
    );
    assert_eq!(
        metadata.0.get("unrelated"),
        Some(&json!({ "preserved": true }))
    );
}

#[test]
fn discover_result_serializes_server_info_in_namespaced_metadata() {
    let mut result = DiscoverResult::new(
        vec![ProtocolVersion::V_2026_07_28],
        rmcp::model::ServerCapabilities::default(),
    );
    result.meta = Some(rmcp::model::MetaObject(
        json!({
            "io.modelcontextprotocol/serverInfo": {
                "name": "stale-server",
                "version": "0.1.0"
            },
            "unrelated": { "preserved": true }
        })
        .as_object()
        .expect("metadata is an object")
        .clone(),
    ));
    result.set_server_info(rmcp::model::Implementation::new("test-server", "1.0.0"));

    let serialized = serde_json::to_value(result).expect("serialize discovery result");
    assert!(serialized.get("serverInfo").is_none());
    assert_eq!(
        serialized["_meta"]["io.modelcontextprotocol/serverInfo"],
        json!({
            "name": "test-server",
            "version": "1.0.0"
        })
    );
    assert_eq!(
        serialized["_meta"]["unrelated"],
        json!({ "preserved": true })
    );
}

#[test]
fn discover_result_ignores_legacy_top_level_server_info() {
    let result: DiscoverResult = serde_json::from_value(json!({
        "resultType": "complete",
        "supportedVersions": ["2026-07-28"],
        "capabilities": {},
        "serverInfo": {
            "name": "top-level-server",
            "version": "2.0.0"
        },
        "ttlMs": 0,
        "cacheScope": "private",
        "_meta": {
            "io.modelcontextprotocol/serverInfo": {
                "name": "metadata-server",
                "version": "1.0.0"
            },
            "unrelated": true
        }
    }))
    .expect("top-level server info should remain supported");

    let server_info = result
        .server_info()
        .expect("namespaced server info is present");
    assert_eq!(server_info.name, "metadata-server");
    assert_eq!(server_info.version, "1.0.0");
    assert_eq!(
        result
            .meta
            .as_ref()
            .and_then(|metadata| metadata.0.get("unrelated")),
        Some(&json!(true))
    );
}

#[test]
fn discover_result_allows_missing_or_malformed_optional_server_info() {
    let result = json!({
        "resultType": "complete",
        "supportedVersions": ["2026-07-28"],
        "capabilities": {},
        "ttlMs": 0,
        "cacheScope": "private",
        "_meta": { "unrelated": true }
    });

    let result =
        serde_json::from_value::<DiscoverResult>(result).expect("server info metadata is optional");
    assert_eq!(result.server_info(), None);

    let malformed_server_info = json!({
        "resultType": "complete",
        "supportedVersions": ["2026-07-28"],
        "capabilities": {},
        "ttlMs": 0,
        "cacheScope": "private",
        "_meta": { "io.modelcontextprotocol/serverInfo": { "name": "missing-version" } }
    });

    let result = serde_json::from_value::<DiscoverResult>(malformed_server_info)
        .expect("opaque metadata should not prevent deserialization");
    assert_eq!(result.server_info(), None);
}

#[test]
fn unsupported_protocol_version_error_matches_draft_schema() {
    let error = ErrorData::unsupported_protocol_version(
        ProtocolVersion::V_2026_07_28,
        &[ProtocolVersion::V_2025_11_25],
    );

    assert_eq!(error.code, ErrorCode::UNSUPPORTED_PROTOCOL_VERSION);
    assert_eq!(
        error.data,
        Some(json!({
            "requested": "2026-07-28",
            "supported": ["2025-11-25"]
        }))
    );
}

#[test]
fn missing_required_capability_error_matches_draft_schema() {
    let required = ClientCapabilities::builder().enable_elicitation().build();
    let error = ErrorData::missing_required_client_capability(required);

    assert_eq!(error.code, ErrorCode::MISSING_REQUIRED_CLIENT_CAPABILITY);
    assert_eq!(
        error.data,
        Some(json!({
            "requiredCapabilities": {
                "elicitation": {}
            }
        }))
    );
}
