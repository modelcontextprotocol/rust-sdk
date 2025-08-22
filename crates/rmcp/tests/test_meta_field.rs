use rmcp::model::{CallToolResult, Content, Meta};
use serde_json::json;

#[test]
fn test_call_tool_result_with_meta() {
    // Test serialization with meta field
    let mut meta = Meta::new();
    meta.0.insert("goose".to_string(), json!({ "displayType": "inline" }));
    
    let result = CallToolResult::success_with_meta(
        vec![Content::text("Hello, world!")],
        meta,
    );
    
    let json = serde_json::to_value(&result).unwrap();
    
    // Verify the _meta field is present and correctly formatted
    assert_eq!(
        json,
        json!({
            "content": [
                {
                    "type": "text",
                    "text": "Hello, world!"
                }
            ],
            "isError": false,
            "_meta": {
                "goose": {
                    "displayType": "inline"
                }
            }
        })
    );
}

#[test]
fn test_call_tool_result_without_meta() {
    // Test that meta field is omitted when None
    let result = CallToolResult::success(vec![Content::text("Hello, world!")]);
    
    let json = serde_json::to_value(&result).unwrap();
    
    // Verify the _meta field is not present
    assert_eq!(
        json,
        json!({
            "content": [
                {
                    "type": "text",
                    "text": "Hello, world!"
                }
            ],
            "isError": false
        })
    );
}

#[test]
fn test_call_tool_result_deserialization_with_meta() {
    // Test deserialization with _meta field
    let json = json!({
        "content": [
            {
                "type": "text",
                "text": "Test content"
            }
        ],
        "isError": false,
        "_meta": {
            "goose": {
                "displayType": "inline"
            },
            "other": "value"
        }
    });
    
    let result: CallToolResult = serde_json::from_value(json).unwrap();
    
    assert_eq!(result.content.len(), 1);
    assert_eq!(result.is_error, Some(false));
    assert!(result.meta.is_some());
    
    let meta = result.meta.unwrap();
    assert_eq!(meta.0.get("goose").unwrap(), &json!({ "displayType": "inline" }));
    assert_eq!(meta.0.get("other").unwrap(), &json!("value"));
}

#[test]
fn test_call_tool_result_deserialization_without_meta() {
    // Test deserialization without _meta field
    let json = json!({
        "content": [
            {
                "type": "text",
                "text": "Test content"
            }
        ],
        "isError": true
    });
    
    let result: CallToolResult = serde_json::from_value(json).unwrap();
    
    assert_eq!(result.content.len(), 1);
    assert_eq!(result.is_error, Some(true));
    assert!(result.meta.is_none());
}
