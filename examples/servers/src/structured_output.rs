//! Example demonstrating structured output from tools
//!
//! This example shows how to:
//! - Return structured data from tools using the Json<T> wrapper
//! - Automatically generate output schemas from Rust types
//! - Handle both structured and unstructured tool outputs

use rmcp::{
    Json, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    tool, tool_handler, tool_router,
    transport::stdio,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct WeatherRequest {
    pub city: String,
    pub units: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct WeatherResponse {
    pub temperature: f64,
    pub description: String,
    pub humidity: u8,
    pub wind_speed: f64,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct CalculationRequest {
    pub numbers: Vec<i32>,
    pub operation: String,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct CalculationResult {
    pub result: f64,
    pub operation: String,
    pub input_count: usize,
}

#[derive(Clone)]
pub struct StructuredOutputServer {
    tool_router: ToolRouter<Self>,
}

#[tool_handler(router = self.tool_router)]
impl rmcp::ServerHandler for StructuredOutputServer {}

impl Default for StructuredOutputServer {
    fn default() -> Self {
        Self::new()
    }
}

#[tool_router(router = tool_router)]
impl StructuredOutputServer {
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }

    /// Get weather information for a city (returns structured data)
    #[tool(name = "get_weather", description = "Get current weather for a city")]
    pub async fn get_weather(
        &self,
        params: Parameters<WeatherRequest>,
    ) -> Result<Json<WeatherResponse>, String> {
        // Simulate weather API call
        let weather = WeatherResponse {
            temperature: match params.0.units.as_deref() {
                Some("fahrenheit") => 72.5,
                _ => 22.5, // celsius by default
            },
            description: "Partly cloudy".to_string(),
            humidity: 65,
            wind_speed: 12.5,
        };

        Ok(Json(weather))
    }

    /// Perform calculations on a list of numbers (returns structured data)
    #[tool(name = "calculate", description = "Perform calculations on numbers")]
    pub async fn calculate(
        &self,
        params: Parameters<CalculationRequest>,
    ) -> Result<Json<CalculationResult>, String> {
        let numbers = &params.0.numbers;
        if numbers.is_empty() {
            return Err("No numbers provided".to_string());
        }

        let result = match params.0.operation.as_str() {
            "sum" => numbers.iter().sum::<i32>() as f64,
            "average" => numbers.iter().sum::<i32>() as f64 / numbers.len() as f64,
            "product" => numbers.iter().product::<i32>() as f64,
            _ => return Err(format!("Unknown operation: {}", params.0.operation)),
        };

        Ok(Json(CalculationResult {
            result,
            operation: params.0.operation,
            input_count: numbers.len(),
        }))
    }

    /// Get server info (returns unstructured text)
    #[tool(name = "get_info", description = "Get server information")]
    pub async fn get_info(&self) -> String {
        "Structured Output Example Server v1.0".to_string()
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    eprintln!("Starting structured output example server...");
    eprintln!();
    eprintln!("This server demonstrates:");
    eprintln!("- Tools that return structured JSON data");
    eprintln!("- Automatic output schema generation");
    eprintln!("- Mixed structured and unstructured outputs");
    eprintln!();
    eprintln!("Tools available:");
    eprintln!("- get_weather: Returns structured weather data");
    eprintln!("- calculate: Returns structured calculation results");
    eprintln!("- get_info: Returns plain text");
    eprintln!();

    let server = StructuredOutputServer::new();

    // Print the tools with their schemas for demonstration
    eprintln!("Tool schemas:");
    for tool in server.tool_router.list_all() {
        eprintln!("\n{}: {}", tool.name, tool.description.unwrap_or_default());
        if let Some(output_schema) = &tool.output_schema {
            eprintln!(
                "  Output schema: {}",
                serde_json::to_string_pretty(output_schema).unwrap()
            );
        } else {
            eprintln!("  Output: Unstructured text");
        }
    }
    eprintln!();

    // Start the server
    eprintln!("Starting server. Connect with an MCP client to test the tools.");
    eprintln!("Press Ctrl+C to stop.");

    let service = server.serve(stdio()).await?;
    service.waiting().await?;

    Ok(())
}
