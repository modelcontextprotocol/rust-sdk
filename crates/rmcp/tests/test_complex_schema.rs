use rmcp::{Error as McpError, model::*, schemars, tool};
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

#[tool(tool_box)]
impl Demo {
    pub fn new() -> Self {
        Self
    }

    #[tool(description = "LLM")]
    async fn chat(
        &self,
        #[tool(aggr)] chat_request: ChatRequest,
    ) -> Result<CallToolResult, McpError> {
        let content = Content::json(chat_request)?;
        Ok(CallToolResult::success(vec![content]))
    }
}

#[test]
fn test_complex_schema() {
    let attr = Demo::chat_tool_attr();
    let input_schema = attr.input_schema;
    let enum_numer = input_schema
        .get("definitions")
        .unwrap()
        .as_object()
        .unwrap()
        .get("ChatRole")
        .unwrap()
        .as_object()
        .unwrap()
        .get("enum")
        .unwrap()
        .as_array()
        .unwrap()
        .len();
    assert_eq!(enum_numer, 4);
    println!("{}", serde_json::to_string_pretty(&input_schema).unwrap());
}
