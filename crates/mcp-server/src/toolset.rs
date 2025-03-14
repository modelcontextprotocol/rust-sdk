use anyhow::Result;
use mcp_core::handler::ToolHandler;
use mcp_core::Tool;
use mcp_core::{Content, ToolError};
use serde_json::Value;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

pub struct Toolset {
    tool_list: HashMap<String, Tool>,
    tool_handlers: HashMap<String, ToolHandlerFn>,
}

pub struct ToolsetBuilder {
    tool_list: HashMap<String, Tool>,
    tool_handlers: HashMap<String, ToolHandlerFn>,
}

impl ToolsetBuilder {
    pub fn new() -> Self {
        Self {
            tool_list: HashMap::new(),
            tool_handlers: HashMap::new(),
        }
    }

    pub fn add_tool(mut self, tool: Tool, handler: ToolHandlerFn) -> Self {
        let name = tool.name.clone();
        self.tool_list.insert(name.clone(), tool);
        self.tool_handlers.insert(name, handler);
        self
    }

    pub fn add_tool_from_handler(mut self, tool_handler: impl ToolHandler) -> Self {
        let name = tool_handler.name().to_string();
        self.tool_list.insert(
            name.clone(),
            Tool {
                name: name.clone(),
                description: tool_handler.description().to_string(),
                input_schema: tool_handler.schema(),
            },
        );

        let tool_handler = Arc::new(tool_handler);
        let handler_fn = Box::new(move |_name: &str, params: Value| {
            let tool_handler = Arc::clone(&tool_handler);
            Box::pin(async move {
                let result = tool_handler.call(params).await?;
                let contents: Vec<Content> = serde_json::from_value(result)
                    .map_err(|e| ToolError::ExecutionError(e.to_string()))?;
                Ok(contents)
            }) as Pin<Box<dyn Future<Output = Result<Vec<Content>, ToolError>> + Send>>
        });

        self.tool_handlers.insert(name, handler_fn);
        self
    }

    pub fn build(self) -> Toolset {
        Toolset {
            tool_list: self.tool_list,
            tool_handlers: self.tool_handlers,
        }
    }
}

impl Default for ToolsetBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl Toolset {
    pub fn builder() -> ToolsetBuilder {
        ToolsetBuilder::new()
    }

    pub fn get_tool(&self, name: &str) -> Option<Tool> {
        self.tool_list.get(name).cloned()
    }

    pub async fn call_tool(
        &self,
        name: &str,
        arguements: Value,
    ) -> Result<Vec<Content>, ToolError> {
        let handler = self
            .tool_handlers
            .get(name)
            .ok_or_else(|| ToolError::NotFound(name.to_string()))?;

        Ok((handler)(name, arguements).await?)
    }

    pub fn list_tools(&self) -> Vec<Tool> {
        self.tool_list.values().cloned().collect()
    }
}

pub type ToolHandlerFn = Box<
    dyn Fn(
        &str,
        Value,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<Content>, ToolError>> + Send + 'static>>,
>;
