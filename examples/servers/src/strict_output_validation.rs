//! Example demonstrating strict output schema validation
//! 
//! This example shows how tools with output_schema must return structured_content

use anyhow::Result;
use rmcp::{
    handler::server::{router::tool::ToolRouter, tool::Parameters, ServerHandler},
    model::CallToolResult,
    service::RoleServer,
    tool, tool_handler, tool_router, ServerCapabilities, Json,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Clone)]
pub struct StrictValidationServer {
    tool_router: ToolRouter<Self>,
}

impl Default for StrictValidationServer {
    fn default() -> Self {
        Self::new()
    }
}

impl StrictValidationServer {
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct CalculationRequest {
    pub a: f64,
    pub b: f64,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct CalculationResult {
    pub result: f64,
    pub operation: String,
}

#[tool_router(router = tool_router)]
impl StrictValidationServer {
    /// This tool has output_schema and returns structured content - will work
    #[tool(name = "add", description = "Add two numbers")]
    pub async fn add(&self, params: Parameters<CalculationRequest>) -> Json<CalculationResult> {
        Json(CalculationResult {
            result: params.0.a + params.0.b,
            operation: "addition".to_string(),
        })
    }

    /// This tool has output_schema but would return regular content - would fail validation
    /// Uncomment to see the validation error:
    // #[tool(name = "bad-add", description = "Add two numbers incorrectly")]
    // pub async fn bad_add(&self, params: Parameters<CalculationRequest>) -> String {
    //     format!("{} + {} = {}", params.0.a, params.0.b, params.0.a + params.0.b)
    // }

    /// This tool manually specifies output_schema and returns CallToolResult directly
    #[tool(
        name = "multiply",
        description = "Multiply two numbers",
        output_schema = std::sync::Arc::new(rmcp::handler::server::tool::schema_for_type::<CalculationResult>())
    )]
    pub async fn multiply(&self, params: Parameters<CalculationRequest>) -> CallToolResult {
        CallToolResult::structured(json!({
            "result": params.0.a * params.0.b,
            "operation": "multiplication"
        }))
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler<RoleServer> for StrictValidationServer {
    fn capabilities(&self) -> &ServerCapabilities {
        &ServerCapabilities {
            tools: Some(Default::default()),
            ..Default::default()
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let server = StrictValidationServer::new();
    
    // List all tools and show their output schemas
    let tools = server.tool_router.list_all();
    for tool in &tools {
        println!("Tool: {}", tool.name);
        if let Some(ref schema) = tool.output_schema {
            println!("  Output Schema: {}", serde_json::to_string_pretty(schema)?);
        }
    }
    
    // Start the server
    println!("\nStarting strict validation server...");
    server.serve(rmcp::transport::Stdio).await?;
    Ok(())
}