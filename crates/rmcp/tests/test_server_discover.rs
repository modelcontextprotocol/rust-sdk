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
