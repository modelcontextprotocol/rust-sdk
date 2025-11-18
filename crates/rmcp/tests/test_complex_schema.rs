use rmcp::{
    ErrorData as McpError, handler::server::wrapper::Parameters, model::*, schemars, tool,
    tool_router,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub enum ChatRole {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ChatRequest {
    pub system: Option<String>,
    pub messages: Vec<ChatMessage>,
}

#[derive(Clone, Default)]
pub struct Demo;

#[tool_router]
impl Demo {
    pub fn new() -> Self {
        Self
    }

    #[tool(description = "LLM")]
    async fn chat(
        &self,
        chat_request: Parameters<ChatRequest>,
    ) -> Result<CallToolResult, McpError> {
        let content = Content::json(chat_request.0)?;
        Ok(CallToolResult::success(vec![content]))
    }
}

fn expected_schema() -> serde_json::Value {
    serde_json::json!({
      "$defs": {
        "ChatMessage": {
          "properties": {
            "content": {
              "type": "string"
            },
            "role": {
              "$ref": "#/$defs/ChatRole"
            }
          },
          "required": [
            "role",
            "content"
          ],
          "type": "object"
        },
        "ChatRole": {
          "enum": [
            "System",
            "User",
            "Assistant",
            "Tool"
          ],
          "type": "string"
        }
      },
      "$schema": "https://json-schema.org/draft/2020-12/schema",
      "properties": {
        "messages": {
          "items": {
            "$ref": "#/$defs/ChatMessage"
          },
          "type": "array"
        },
        "system": {
          "nullable": true,
          "type": "string"
        }
      },
      "required": [
        "messages"
      ],
      "title": "ChatRequest",
      "type": "object"
    })
}

#[test]
fn test_complex_schema() {
    let attr = Demo::chat_tool_attr();
    let input_schema = attr.input_schema;
    let expected = expected_schema();
    let produced = serde_json::Value::Object(input_schema.as_ref().clone());
    assert_eq!(produced, expected, "schema mismatch");
}
