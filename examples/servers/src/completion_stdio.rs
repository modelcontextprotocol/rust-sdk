//! MCP Server demonstrating completion functionality
//!
//! This example shows how to create an MCP server that advertises completion
//! support and demonstrates the basic completion capability.
//!
//! Run with MCP Inspector:
//! ```bash
//! npx @modelcontextprotocol/inspector cargo run -p mcp-server-examples --example servers_completion_stdio
//! ```

use anyhow::Result;
use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler, ServiceExt,
    handler::server::{completion::DefaultCompletionProvider, wrapper::Parameters},
    model::*,
    prompt,
    schemars::JsonSchema,
    service::RequestContext,
};
use serde::{Deserialize, Serialize};
use tracing_subscriber::{self, EnvFilter};

/// Arguments for the weather query prompt
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "Weather query parameters")]
pub struct WeatherQueryArgs {
    /// Country name (supports completion)
    #[serde(default)]
    #[schemars(description = "Country name where the city is located")]
    pub country: String,

    /// City name (supports context-aware completion)
    #[serde(default)]
    #[schemars(description = "City name for weather query")]
    pub city: String,

    /// Temperature units
    #[serde(default)]
    #[schemars(description = "Temperature units (celsius, fahrenheit, kelvin)")]
    pub units: Option<String>,
}

/// MCP Server that demonstrates completion functionality
#[derive(Clone)]
pub struct CompletionDemoServer {
    completion_provider: DefaultCompletionProvider,
}

impl Default for CompletionDemoServer {
    fn default() -> Self {
        Self {
            completion_provider: DefaultCompletionProvider::new(),
        }
    }
}

// Weather query prompt with completion support
#[prompt(
    name = "weather_query",
    description = "Get current weather for a specific location with smart completion support for country and city fields"
)]
pub async fn weather_query_prompt(
    Parameters(args): Parameters<WeatherQueryArgs>,
) -> Result<PromptMessage, McpError> {
    let units = args.units.unwrap_or_else(|| "celsius".to_string());

    let prompt_text = if args.country.is_empty() || args.city.is_empty() {
        "Please specify both a country and city to get weather information.".to_string()
    } else {
        format!(
            "Please provide the current weather for {}, {} in {}. Include temperature, humidity, wind conditions, and a brief description of the current conditions.",
            args.city, args.country, units
        )
    };

    Ok(PromptMessage::new_text(
        PromptMessageRole::User,
        prompt_text,
    ))
}

impl ServerHandler for CompletionDemoServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            capabilities: ServerCapabilities::builder()
                .enable_completions() // Enable completion capability
                .enable_prompts()
                .build(),
            instructions: Some(
                "Weather MCP Server with Completion Support\n\n\
                This server provides a weather query prompt with completion support.\n\
                The server advertises completion capability in its capabilities.\n\n\
                Prompts:\n\
                • weather_query: Get current weather (supports completion for country/city/units)\n\n\
                Try using completion/complete requests to get suggestions for prompt arguments!"
                    .to_string(),
            ),
            ..Default::default()
        }
    }

    // Demonstrate completion using standard DefaultCompletionProvider
    async fn complete(
        &self,
        request: CompleteRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> Result<CompleteResult, McpError> {
        // Get candidates for weather_query prompt arguments
        let candidates = match &request.r#ref {
            Reference::Prompt(prompt_ref) => {
                if prompt_ref.name == "weather_query" {
                    match request.argument.name.as_str() {
                        "country" => vec![
                            "USA",
                            "France",
                            "Germany",
                            "Japan",
                            "United Kingdom",
                            "Canada",
                            "Australia",
                            "Italy",
                            "Spain",
                            "Brazil",
                        ],
                        "city" => vec![
                            "New York",
                            "Los Angeles",
                            "Chicago",
                            "Houston",
                            "San Francisco",
                            "Las Vegas",
                            "San Diego",
                            "San Antonio",
                            "New Orleans",
                            "Salt Lake City",
                            "Paris",
                            "Lyon",
                            "Marseille",
                            "Berlin",
                            "Munich",
                            "Frankfurt am Main",
                            "Tokyo",
                            "Osaka",
                            "Kyoto",
                            "London",
                            "Toronto",
                            "Sydney",
                            "Buenos Aires",
                            "Mexico City",
                            "Rio de Janeiro",
                            "São Paulo",
                            "Hong Kong",
                            "Amsterdam",
                            "Beijing",
                            "Shanghai",
                            "Guangzhou",
                            "Shenzhen",
                            "Chengdu",
                            "Hangzhou",
                        ],
                        "units" => vec!["celsius", "fahrenheit", "kelvin"],
                        _ => vec!["example_value", "sample_input"],
                    }
                } else {
                    vec!["example_value", "sample_input"]
                }
            }
            Reference::Resource(_) => vec!["resource_example", "resource_sample"],
        };

        // Convert &str to String for fuzzy matching
        let string_candidates: Vec<String> =
            candidates.into_iter().map(|s| s.to_string()).collect();

        // Use standard fuzzy matching from DefaultCompletionProvider
        let suggestions = self
            .completion_provider
            .fuzzy_match(&request.argument.value, &string_candidates);

        let completion = CompletionInfo {
            values: suggestions,
            total: None,
            has_more: Some(false),
        };

        Ok(CompleteResult { completion })
    }

    async fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParam>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, McpError> {
        let prompts = vec![Prompt {
            name: "weather_query".to_string(),
            description: Some(
                "Get current weather for a specific location with completion support".to_string(),
            ),
            arguments: Some(vec![
                PromptArgument {
                    name: "country".to_string(),
                    description: Some("Country name where the city is located".to_string()),
                    required: Some(false),
                },
                PromptArgument {
                    name: "city".to_string(),
                    description: Some("City name for weather query".to_string()),
                    required: Some(false),
                },
                PromptArgument {
                    name: "units".to_string(),
                    description: Some(
                        "Temperature units (celsius, fahrenheit, kelvin)".to_string(),
                    ),
                    required: Some(false),
                },
            ]),
        }];

        Ok(ListPromptsResult {
            prompts,
            next_cursor: None,
        })
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResult, McpError> {
        match request.name.as_str() {
            "weather_query" => {
                let args: WeatherQueryArgs = serde_json::from_value(
                    request
                        .arguments
                        .map(serde_json::Value::Object)
                        .unwrap_or(serde_json::Value::Object(serde_json::Map::new())),
                )
                .map_err(|e| McpError::invalid_params(e.to_string(), None))?;
                let prompt = weather_query_prompt(Parameters(args)).await?;
                Ok(GetPromptResult {
                    description: Some("Weather query prompt".to_string()),
                    messages: vec![prompt],
                })
            }
            _ => Err(McpError::invalid_params(
                format!("Unknown prompt: {}", request.name),
                None,
            )),
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive(tracing::Level::INFO.into()))
        .with_writer(std::io::stderr)
        .init();

    tracing::info!("Starting MCP Completion Demo Server");
    tracing::info!("Features:");
    tracing::info!("  • Single weather_query prompt with completion support");
    tracing::info!("  • Uses standard DefaultCompletionProvider");
    tracing::info!("  • Advanced fuzzy matching with acronym support");

    // Create server with completion support
    let server = CompletionDemoServer::default();

    // Serve on stdio transport
    server
        .serve(rmcp::transport::stdio())
        .await?
        .waiting()
        .await?;

    Ok(())
}
