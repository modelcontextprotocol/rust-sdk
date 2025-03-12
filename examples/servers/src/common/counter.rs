use mcp_core::protocol::{JsonRpcRequest, JsonRpcResponse};
use mcp_core::{
    handler::{PromptError, ResourceError},
    prompt::{Prompt, PromptArgument},
    protocol::ServerCapabilities,
    Content, Resource, Tool, ToolError,
};
use mcp_server::router::CapabilitiesBuilder;
use mcp_server::{BoxError, Router, RouterError};
use serde_json::Value;
use std::task::{Context, Poll};
use std::{future::Future, pin::Pin, sync::Arc};
use tokio::sync::Mutex;
use tower_service::Service;


#[derive(Clone)]
pub struct CounterRouter {
    counter: Arc<Mutex<i32>>,
}

impl CounterRouter {
    pub fn new() -> Self {
        Self {
            counter: Arc::new(Mutex::new(0)),
        }
    }

    async fn increment(&self) -> Result<i32, ToolError> {
        let mut counter = self.counter.lock().await;
        *counter += 1;
        Ok(*counter)
    }

    async fn decrement(&self) -> Result<i32, ToolError> {
        let mut counter = self.counter.lock().await;
        *counter -= 1;
        Ok(*counter)
    }

    async fn get_value(&self) -> Result<i32, ToolError> {
        let counter = self.counter.lock().await;
        Ok(*counter)
    }

    fn _create_resource_text(&self, uri: &str, name: &str) -> Resource {
        Resource::new(uri, Some("text/plain".to_string()), Some(name.to_string())).unwrap()
    }
}

impl mcp_server::Router for CounterRouter {
    fn name(&self) -> String {
        "counter".to_string()
    }

    fn instructions(&self) -> String {
        "This server provides a counter tool that can increment and decrement values. The counter starts at 0 and can be modified using the 'increment' and 'decrement' tools. Use 'get_value' to check the current count.".to_string()
    }

    fn capabilities(&self) -> ServerCapabilities {
        CapabilitiesBuilder::new()
            .with_tools(false)
            .with_resources(false, false)
            .with_prompts(false)
            .build()
    }

    fn list_tools(&self) -> Vec<Tool> {
        vec![
            Tool::new(
                "increment".to_string(),
                "Increment the counter by 1".to_string(),
                serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
                }),
            ),
            Tool::new(
                "decrement".to_string(),
                "Decrement the counter by 1".to_string(),
                serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
                }),
            ),
            Tool::new(
                "get_value".to_string(),
                "Get the current counter value".to_string(),
                serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
                }),
            ),
        ]
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
                "increment" => {
                    let value = this.increment().await?;
                    Ok(vec![Content::text(value.to_string())])
                }
                "decrement" => {
                    let value = this.decrement().await?;
                    Ok(vec![Content::text(value.to_string())])
                }
                "get_value" => {
                    let value = this.get_value().await?;
                    Ok(vec![Content::text(value.to_string())])
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

pub struct RouterService<T>(pub T);

impl<T> Service<JsonRpcRequest> for RouterService<T>
where
    T: Router + Clone + Send + Sync + 'static,
{
    type Response = JsonRpcResponse;
    type Error = BoxError;
    type Future = Pin<Box<dyn Future<Output =std::result::Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<std::result::Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: JsonRpcRequest) -> Self::Future {
        let this = self.0.clone();

        Box::pin(async move {
            let result = match req.method.as_str() {
                "initialize" => this.handle_initialize(req).await,
                "tools/list" => this.handle_tools_list(req).await,
                "tools/call" => this.handle_tools_call(req).await,
                "resources/list" => this.handle_resources_list(req).await,
                "resources/read" => this.handle_resources_read(req).await,
                "prompts/list" => this.handle_prompts_list(req).await,
                "prompts/get" => this.handle_prompts_get(req).await,
                _ => {
                    let mut response = this.create_response(req.id);
                    response.error = Some(RouterError::MethodNotFound(req.method).into());
                    Ok(response)
                }
            };

            result.map_err(BoxError::from)
        })
    }
}
