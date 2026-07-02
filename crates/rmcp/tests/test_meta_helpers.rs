#![allow(deprecated)]

use rmcp::model::{ClientCapabilities, Implementation, LoggingLevel, Meta, ProtocolVersion};
use serde_json::json;

const META_KEY_PROTOCOL_VERSION: &str = "io.modelcontextprotocol/protocolVersion";
const META_KEY_CLIENT_INFO: &str = "io.modelcontextprotocol/clientInfo";
const META_KEY_CLIENT_CAPABILITIES: &str = "io.modelcontextprotocol/clientCapabilities";
const META_KEY_LOG_LEVEL: &str = "io.modelcontextprotocol/logLevel";

#[test]
fn meta_setters_store_sep_2575_values() {
    let mut meta = Meta::new();
    meta.set_protocol_version(ProtocolVersion::V_2026_07_28);
    meta.set_client_info(Implementation::new("test-client", "1.0.0"));
    meta.set_client_capabilities(ClientCapabilities::default());
    meta.set_log_level(LoggingLevel::Warning);

    assert_eq!(
        meta.get(META_KEY_PROTOCOL_VERSION),
        Some(&json!("2026-07-28"))
    );
    assert_eq!(
        meta.get(META_KEY_CLIENT_INFO),
        Some(&json!({ "name": "test-client", "version": "1.0.0" }))
    );
    assert_eq!(meta.get(META_KEY_CLIENT_CAPABILITIES), Some(&json!({})));
    assert_eq!(meta.get(META_KEY_LOG_LEVEL), Some(&json!("warning")));
}

#[test]
fn meta_accessors_decode_wire_values() {
    let meta: Meta = serde_json::from_value(json!({
        "progressToken": "progress-1",
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientInfo": {
            "name": "wire-client",
            "version": "9.0.0"
        },
        "io.modelcontextprotocol/clientCapabilities": {
            "sampling": {}
        },
        "io.modelcontextprotocol/logLevel": "error"
    }))
    .unwrap();

    assert_eq!(meta.protocol_version(), Some(ProtocolVersion::V_2026_07_28));
    assert_eq!(
        meta.client_info(),
        Some(Implementation::new("wire-client", "9.0.0"))
    );
    assert!(
        meta.client_capabilities()
            .is_some_and(|capabilities| capabilities.sampling.is_some())
    );
    assert_eq!(meta.log_level(), Some(LoggingLevel::Error));
}

#[test]
fn meta_accessors_ignore_missing_or_malformed_values() {
    let meta: Meta = serde_json::from_value(json!({
        "io.modelcontextprotocol/protocolVersion": 20260728,
        "io.modelcontextprotocol/clientInfo": "not an implementation",
        "io.modelcontextprotocol/clientCapabilities": "not capabilities",
        "io.modelcontextprotocol/logLevel": "loud"
    }))
    .unwrap();

    assert_eq!(meta.protocol_version(), None);
    assert_eq!(meta.client_info(), None);
    assert_eq!(meta.client_capabilities(), None);
    assert_eq!(meta.log_level(), None);
}
