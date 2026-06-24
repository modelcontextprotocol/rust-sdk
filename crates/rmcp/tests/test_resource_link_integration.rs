/// Integration tests for resource_link support in both tools and prompts
use rmcp::model::{CallToolResult, ContentBlock, PromptMessage, Resource, Role};

#[test]
fn test_tool_and_prompt_resource_link_compatibility() {
    let resource = Resource::new("file:///shared/data.json", "Shared Data");

    // Test 1: Tool returning a resource link
    let tool_result = CallToolResult::success(vec![
        ContentBlock::text("Found shared data"),
        ContentBlock::resource_link(resource.clone()),
    ]);

    let tool_json = serde_json::to_string(&tool_result).unwrap();
    assert!(tool_json.contains("\"type\":\"resource_link\""));

    // Test 2: Prompt returning a resource link
    let prompt_message = PromptMessage::new_resource_link(Role::Assistant, resource.clone());

    let prompt_json = serde_json::to_string(&prompt_message).unwrap();
    assert!(prompt_json.contains("\"type\":\"resource_link\""));

    // Test 3: Verify both serialize to the same resource link structure
    let tool_content = &tool_result.content[1];
    let prompt_content = &prompt_message.content;

    let tool_resource_json = serde_json::to_value(tool_content).unwrap();
    let prompt_resource_json = serde_json::to_value(prompt_content).unwrap();

    assert_eq!(
        tool_resource_json.get("type").unwrap(),
        prompt_resource_json.get("type").unwrap()
    );
    assert_eq!(
        tool_resource_json.get("uri").unwrap(),
        prompt_resource_json.get("uri").unwrap()
    );
    assert_eq!(
        tool_resource_json.get("name").unwrap(),
        prompt_resource_json.get("name").unwrap()
    );
}

#[test]
fn test_resource_link_roundtrip() {
    let resource = Resource::new("https://api.example.com/resource", "API Resource")
        .with_description("External API resource")
        .with_mime_type("application/json")
        .with_size(2048);

    // Test with tool result
    let tool_result = CallToolResult::success(vec![ContentBlock::resource_link(resource.clone())]);

    let tool_json = serde_json::to_string(&tool_result).unwrap();
    let tool_deserialized: CallToolResult = serde_json::from_str(&tool_json).unwrap();

    if let Some(resource_link) = tool_deserialized.content[0].as_resource_link() {
        assert_eq!(resource_link.uri, "https://api.example.com/resource");
        assert_eq!(resource_link.name, "API Resource");
        assert_eq!(
            resource_link.description,
            Some("External API resource".to_string())
        );
        assert_eq!(
            resource_link.mime_type,
            Some("application/json".to_string())
        );
        assert_eq!(resource_link.size, Some(2048));
    } else {
        panic!("Expected resource link in tool result");
    }

    // Test with prompt message
    let prompt_message = PromptMessage::new(Role::User, ContentBlock::resource_link(resource));

    let prompt_json = serde_json::to_string(&prompt_message).unwrap();
    let prompt_deserialized: PromptMessage = serde_json::from_str(&prompt_json).unwrap();

    if let ContentBlock::ResourceLink(link) = &prompt_deserialized.content {
        assert_eq!(link.uri, "https://api.example.com/resource");
        assert_eq!(link.name, "API Resource");
        assert_eq!(link.description, Some("External API resource".to_string()));
        assert_eq!(link.mime_type, Some("application/json".to_string()));
        assert_eq!(link.size, Some(2048));
    } else {
        panic!("Expected resource link in prompt message");
    }
}

#[test]
fn test_mixed_content_in_prompts_and_tools() {
    let resource1 = Resource::new("file:///doc1.md", "Document 1");
    let resource2 = Resource::new("file:///doc2.md", "Document 2");

    // Tool with mixed content
    let tool_result = CallToolResult::success(vec![
        ContentBlock::text("Processing complete. Found documents:"),
        ContentBlock::resource_link(resource1),
        ContentBlock::resource_link(resource2),
        ContentBlock::embedded_text("summary://result", "Both documents processed successfully"),
    ]);

    assert_eq!(tool_result.content.len(), 4);
    assert!(tool_result.content[0].as_text().is_some());
    assert!(tool_result.content[1].as_resource_link().is_some());
    assert!(tool_result.content[2].as_resource_link().is_some());
    assert!(tool_result.content[3].as_resource().is_some());

    // Verify serialization includes all types
    let json = serde_json::to_string(&tool_result).unwrap();
    assert!(json.contains("\"type\":\"text\""));
    assert!(json.contains("\"type\":\"resource_link\""));
    assert!(json.contains("\"type\":\"resource\""));
}
