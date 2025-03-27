use std::{future::Future, pin::Pin};

use mcp_core::{
    handler::{PromptError, ResourceError},
    prompt::{Prompt, PromptArgument},
    protocol::ServerCapabilities,
    Content, Resource, Tool, ToolError,
};
use mcp_server::router::CapabilitiesBuilder;
use serde_json::Value;

use crate::shiitake_scraper::shiitake_uranai_scraper::scrape;

#[derive(Clone)]
pub struct ShiitakeUranaiRouter {
    constellation: String,
}

impl ShiitakeUranaiRouter {
    pub fn new(constellation: String) -> Self {
        Self { constellation }
    }

    async fn fetch_fortune(&self) -> Result<String, ToolError> {
        match scrape(self.constellation.clone()).await {
            Ok(fortune) => Ok(fortune),
            Err(e) => Err(ToolError::ExecutionError(format!(
                "Failed to fetch fortune: {}",
                e
            ))),
        }
    }

    fn _create_resource_text(&self, uri: &str, name: &str) -> Resource {
        Resource::new(uri, Some("text/plain".to_string()), Some(name.to_string())).unwrap()
    }
}

impl mcp_server::Router for ShiitakeUranaiRouter {
    fn name(&self) -> String {
        "しいたけ占いアドバイザー".to_string()
    }

    fn instructions(&self) -> String {
        "今週のしいたけ占いの結果を返します".to_string()
    }

    fn capabilities(&self) -> ServerCapabilities {
        CapabilitiesBuilder::new()
            .with_tools(false)
            .with_resources(false, false)
            .with_prompts(false)
            .build()
    }

    fn list_tools(&self) -> Vec<Tool> {
        vec![Tool::new(
            "fetch_fortune".to_string(),
            "今週のしいたけ占いの内容を取得します".to_string(),
            serde_json::json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        )]
    }

    fn call_tool(
        &self,
        tool_name: &str,
        _arguments: Value,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<Content>, ToolError>> + Send + 'static>> {
        let this = self.clone();
        let tool_name = tool_name.to_string();

        Box::pin(async move {
            match tool_name.as_str() {
                "fetch_fortune" => {
                    let fortune = this.fetch_fortune().await?;
                    Ok(vec![Content::text(fortune)])
                }
                _ => Err(ToolError::NotFound(format!("Tool {} not found", tool_name))),
            }
        })
    }

    fn list_resources(&self) -> Vec<Resource> {
        vec![
            self._create_resource_text("str:////Users/to/some/path/", "cwd"),
            self._create_resource_text("memo://insights", "memo-name"),
        ]
    }

    fn read_resource(
        &self,
        uri: &str,
    ) -> Pin<Box<dyn Future<Output = Result<String, ResourceError>> + Send + 'static>> {
        let uri = uri.to_string();
        Box::pin(async move {
            match uri.as_str() {
                "str:////Users/to/some/path/" => {
                    let cwd = "/Users/to/some/path/";
                    Ok(cwd.to_string())
                }
                "memo://insights" => {
                    let memo =
                        "Business Intelligence Memo\n\nAnalysis has revealed 5 key insights ...";
                    Ok(memo.to_string())
                }
                _ => Err(ResourceError::NotFound(format!(
                    "Resource {} not found",
                    uri
                ))),
            }
        })
    }

    fn list_prompts(&self) -> Vec<Prompt> {
        vec![Prompt::new(
            "example_prompt",
            Some("This is an example prompt that takes one required agrument, message"),
            Some(vec![PromptArgument {
                name: "message".to_string(),
                description: Some("A message to put in the prompt".to_string()),
                required: Some(true),
            }]),
        )]
    }

    fn get_prompt(
        &self,
        prompt_name: &str,
    ) -> Pin<Box<dyn Future<Output = Result<String, PromptError>> + Send + 'static>> {
        let prompt_name = prompt_name.to_string();
        Box::pin(async move {
            match prompt_name.as_str() {
                "example_prompt" => {
                    let prompt = "This is an example prompt with your message here: '{message}'";
                    Ok(prompt.to_string())
                }
                _ => Err(PromptError::NotFound(format!(
                    "Prompt {} not found",
                    prompt_name
                ))),
            }
        })
    }
}
