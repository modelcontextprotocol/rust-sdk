use rmcp::model::{JsonObject, ListToolsResult, Tool};
use serde_json::json;

#[test]
fn constructors_omit_unset_security_schemes() {
    let tools = [
        Tool::default(),
        Tool::new("search", "Search documents", JsonObject::new()),
        Tool::new_with_raw("search", None, JsonObject::new()),
    ];

    for tool in tools {
        assert!(tool.security_schemes.is_none());

        let wire = serde_json::to_value(tool).unwrap();
        assert!(wire.get("securitySchemes").is_none());
        assert!(wire.get("security_schemes").is_none());
    }
}

#[test]
fn tools_without_security_schemes_keep_the_existing_wire_shape() {
    let wire = json!({
        "name": "search",
        "description": "Search documents",
        "inputSchema": {"type": "object"}
    });
    let tool: Tool = serde_json::from_value(wire.clone()).unwrap();

    assert!(tool.security_schemes.is_none());
    assert_eq!(serde_json::to_value(tool).unwrap(), wire);
}

#[test]
fn list_tools_preserves_noauth_and_oauth2_schemes() {
    let schemes = json!([
        {"type": "noauth"},
        {"type": "oauth2", "scopes": ["documents.read", "documents.write"]}
    ]);
    let tool = Tool::new("search", "Search documents", JsonObject::new())
        .with_security_schemes(serde_json::from_value(schemes.clone()).unwrap());
    let result = ListToolsResult::with_all_items(vec![tool]);

    let wire = serde_json::to_value(&result).unwrap();
    assert_eq!(wire["tools"][0]["securitySchemes"], schemes);
    assert!(wire["tools"][0].get("_meta").is_none());
    assert!(wire["tools"][0].get("security_schemes").is_none());

    let restored: ListToolsResult = serde_json::from_value(wire).unwrap();
    assert_eq!(restored, result);
}

#[test]
fn security_schemes_and_application_metadata_remain_independent() {
    let wire = json!({
        "name": "search",
        "inputSchema": {"type": "object"},
        "securitySchemes": [{"type": "oauth2", "scopes": ["documents.read"]}],
        "_meta": {"application/example": "retained"}
    });
    let tool: Tool = serde_json::from_value(wire.clone()).unwrap();

    assert_eq!(tool.security_schemes.as_ref().unwrap().len(), 1);
    assert_eq!(serde_json::to_value(tool).unwrap(), wire);
}
