#![allow(dead_code)]
use std::sync::Arc;

use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler,
    handler::server::{
        router::{prompt::PromptRouter, tool::ToolRouter},
        wrapper::Parameters,
    },
    model::*,
    prompt, prompt_handler, prompt_router, schemars,
    service::RequestContext,
    tool, tool_handler, tool_router,
};
use serde_json::json;
use tokio::sync::Mutex;

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct StructRequest {
    pub a: i32,
    pub b: i32,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct ExamplePromptArgs {
    /// A message to put in the prompt
    pub message: String,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct CounterAnalysisArgs {
    /// The target value you're trying to reach
    pub goal: i32,
    /// Preferred strategy: 'fast' or 'careful'
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strategy: Option<String>,
}

#[derive(Clone)]
pub struct Counter {
    counter: Arc<Mutex<i32>>,
    tool_router: ToolRouter<Counter>,
    prompt_router: PromptRouter<Counter>,
}

#[tool_router]
impl Counter {
    #[allow(dead_code)]
    pub fn new() -> Self {
        Self {
            counter: Arc::new(Mutex::new(0)),
            tool_router: Self::tool_router(),
            prompt_router: Self::prompt_router(),
        }
    }

    fn _create_resource_text(&self, uri: &str, name: &str) -> Resource {
        RawResource::new(uri, name.to_string()).no_annotation()
    }

    #[tool(description = "Increment the counter by 1")]
    async fn increment(&self) -> Result<CallToolResult, McpError> {
        let mut counter = self.counter.lock().await;
        *counter += 1;
        Ok(CallToolResult::success(vec![Content::text(
            counter.to_string(),
        )]))
    }

    #[tool(description = "Decrement the counter by 1")]
    async fn decrement(&self) -> Result<CallToolResult, McpError> {
        let mut counter = self.counter.lock().await;
        *counter -= 1;
        Ok(CallToolResult::success(vec![Content::text(
            counter.to_string(),
        )]))
    }

    #[tool(description = "Get the current counter value")]
    async fn get_value(&self) -> Result<CallToolResult, McpError> {
        let counter = self.counter.lock().await;
        Ok(CallToolResult::success(vec![Content::text(
            counter.to_string(),
        )]))
    }

    #[tool(description = "Say hello to the client")]
    fn say_hello(&self) -> Result<CallToolResult, McpError> {
        Ok(CallToolResult::success(vec![Content::text("hello")]))
    }

    #[tool(description = "Repeat what you say")]
    fn echo(&self, Parameters(object): Parameters<JsonObject>) -> Result<CallToolResult, McpError> {
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::Value::Object(object).to_string(),
        )]))
    }

    #[tool(description = "Calculate the sum of two numbers")]
    fn sum(
        &self,
        Parameters(StructRequest { a, b }): Parameters<StructRequest>,
    ) -> Result<CallToolResult, McpError> {
        Ok(CallToolResult::success(vec![Content::text(
            (a + b).to_string(),
        )]))
    }
}

#[prompt_router]
impl Counter {
    /// This is an example prompt that takes one required argument, message
    #[prompt(name = "example_prompt")]
    async fn example_prompt(
        &self,
        Parameters(args): Parameters<ExamplePromptArgs>,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<Vec<PromptMessage>, McpError> {
        let prompt = format!(
            "This is an example prompt with your message here: '{}'",
            args.message
        );
        Ok(vec![PromptMessage {
            role: PromptMessageRole::User,
            content: PromptMessageContent::text(prompt),
        }])
    }

    /// Analyze the current counter value and suggest next steps
    #[prompt(name = "counter_analysis")]
    async fn counter_analysis(
        &self,
        Parameters(args): Parameters<CounterAnalysisArgs>,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<GetPromptResult, McpError> {
        let strategy = args.strategy.unwrap_or_else(|| "careful".to_string());
        let current_value = *self.counter.lock().await;
        let difference = args.goal - current_value;

        let messages = vec![
            PromptMessage::new_text(
                PromptMessageRole::Assistant,
                "I'll analyze the counter situation and suggest the best approach.",
            ),
            PromptMessage::new_text(
                PromptMessageRole::User,
                format!(
                    "Current counter value: {}\nGoal value: {}\nDifference: {}\nStrategy preference: {}\n\nPlease analyze the situation and suggest the best approach to reach the goal.",
                    current_value, args.goal, difference, strategy
                ),
            ),
        ];

        Ok(GetPromptResult {
            description: Some(format!(
                "Counter analysis for reaching {} from {}",
                args.goal, current_value
            )),
            messages,
        })
    }
}

#[tool_handler]
#[prompt_handler]
impl ServerHandler for Counter {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::V_2024_11_05,
            capabilities: ServerCapabilities::builder()
                .enable_prompts()
                .enable_resources()
                .enable_tools()
                .build(),
            server_info: Implementation::from_build_env(),
            instructions: Some("This server provides counter tools and prompts. Tools: increment, decrement, get_value, say_hello, echo, sum. Prompts: example_prompt (takes a message), counter_analysis (analyzes counter state with a goal).".to_string()),
        }
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParam>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        Ok(ListResourcesResult {
            resources: vec![
                self._create_resource_text("str:////Users/to/some/path/", "cwd"),
                self._create_resource_text("memo://insights", "memo-name"),
            ],
            next_cursor: None,
        })
    }

    async fn read_resource(
        &self,
        ReadResourceRequestParam { uri }: ReadResourceRequestParam,
        _: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, McpError> {
        match uri.as_str() {
            "str:////Users/to/some/path/" => {
                let cwd = "/Users/to/some/path/";
                Ok(ReadResourceResult {
                    contents: vec![ResourceContents::text(cwd, uri)],
                })
            }
            "memo://insights" => {
                let memo = "Business Intelligence Memo\n\nAnalysis has revealed 5 key insights ...";
                Ok(ReadResourceResult {
                    contents: vec![ResourceContents::text(memo, uri)],
                })
            }
            _ => Err(McpError::resource_not_found(
                "resource_not_found",
                Some(json!({
                    "uri": uri
                })),
            )),
        }
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParam>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, McpError> {
        Ok(ListResourceTemplatesResult {
            next_cursor: None,
            resource_templates: Vec::new(),
        })
    }

    async fn initialize(
        &self,
        _request: InitializeRequestParam,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, McpError> {
        if let Some(http_request_part) = context.extensions.get::<axum::http::request::Parts>() {
            let initialize_headers = &http_request_part.headers;
            let initialize_uri = &http_request_part.uri;
            tracing::info!(?initialize_headers, %initialize_uri, "initialize from http server");
        }
        Ok(self.get_info())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_prompt_attributes_generated() {
        // Verify that the prompt macros generate the expected attributes
        let example_attr = Counter::example_prompt_prompt_attr();
        assert_eq!(example_attr.name, "example_prompt");
        assert!(example_attr.description.is_some());
        assert!(example_attr.arguments.is_some());

        let args = example_attr.arguments.unwrap();
        assert_eq!(args.len(), 1);
        assert_eq!(args[0].name, "message");
        assert_eq!(args[0].required, Some(true));

        let analysis_attr = Counter::counter_analysis_prompt_attr();
        assert_eq!(analysis_attr.name, "counter_analysis");
        assert!(analysis_attr.description.is_some());
        assert!(analysis_attr.arguments.is_some());

        let args = analysis_attr.arguments.unwrap();
        assert_eq!(args.len(), 2);
        assert_eq!(args[0].name, "goal");
        assert_eq!(args[0].required, Some(true));
        assert_eq!(args[1].name, "strategy");
        assert_eq!(args[1].required, Some(false));
    }

    #[tokio::test]
    async fn test_prompt_router_has_routes() {
        let router = Counter::prompt_router();
        assert!(router.has_route("example_prompt"));
        assert!(router.has_route("counter_analysis"));

        let prompts = router.list_all();
        assert_eq!(prompts.len(), 2);
    }

    #[tokio::test]
    async fn test_example_prompt_execution() {
        let counter = Counter::new();
        let context = rmcp::handler::server::prompt::PromptContext::new(
            &counter,
            "example_prompt".to_string(),
            Some({
                let mut map = serde_json::Map::new();
                map.insert(
                    "message".to_string(),
                    serde_json::Value::String("Test message".to_string()),
                );
                map
            }),
            RequestContext {
                meta: Default::default(),
                ct: tokio_util::sync::CancellationToken::new(),
                id: rmcp::model::NumberOrString::String("test-1".to_string()),
                peer: Default::default(),
                extensions: Default::default(),
            },
        );

        let router = Counter::prompt_router();
        let result = router.get_prompt(context).await;
        assert!(result.is_ok());

        let prompt_result = result.unwrap();
        assert_eq!(prompt_result.messages.len(), 1);
        assert_eq!(prompt_result.messages[0].role, PromptMessageRole::User);
    }
}
