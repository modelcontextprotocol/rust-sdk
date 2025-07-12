use anyhow::Result;
use rmcp::{
    Error as McpError, RoleServer, ServerHandler, ServiceExt,
    handler::server::prompt::arguments_from_schema, model::*, schemars, service::RequestContext,
    transport::stdio,
};
use serde::{Deserialize, Serialize};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
struct CodeReviewArgs {
    /// The file path to review
    file_path: String,
    /// Language for syntax highlighting
    #[serde(default = "default_language")]
    language: String,
}

fn default_language() -> String {
    "rust".to_string()
}

#[derive(Clone, Debug, Default)]
struct PromptExampleServer;

impl ServerHandler for PromptExampleServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            server_info: Implementation {
                name: "Prompt Example Server".to_string(),
                version: "1.0.0".to_string(),
            },
            instructions: Some(
                concat!(
                    "This server demonstrates the prompt framework capabilities. ",
                    "It provides code review and debugging prompts."
                )
                .to_string(),
            ),
            capabilities: ServerCapabilities::builder().enable_prompts().build(),
            ..Default::default()
        }
    }

    async fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParam>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, McpError> {
        Ok(ListPromptsResult {
            next_cursor: None,
            prompts: vec![
                Prompt {
                    name: "code_review".to_string(),
                    description: Some(
                        "Reviews code for best practices and potential issues".to_string(),
                    ),
                    arguments: arguments_from_schema::<CodeReviewArgs>(),
                },
                Prompt {
                    name: "debug_helper".to_string(),
                    description: Some("Interactive debugging assistant".to_string()),
                    arguments: None,
                },
            ],
        })
    }

    async fn get_prompt(
        &self,
        GetPromptRequestParam { name, arguments }: GetPromptRequestParam,
        _: RequestContext<RoleServer>,
    ) -> Result<GetPromptResult, McpError> {
        match name.as_str() {
            "code_review" => {
                // Parse arguments
                let args = if let Some(args_map) = arguments {
                    serde_json::from_value::<CodeReviewArgs>(serde_json::Value::Object(args_map))
                        .map_err(|e| {
                            McpError::invalid_params(format!("Invalid arguments: {}", e), None)
                        })?
                } else {
                    return Err(McpError::invalid_params("Missing required arguments", None));
                };

                Ok(GetPromptResult {
                    description: None,
                    messages: vec![
                        PromptMessage::new_text(
                            PromptMessageRole::User,
                            format!(
                                "Please review the {} code in file: {}",
                                args.language, args.file_path
                            ),
                        ),
                        PromptMessage::new_text(
                            PromptMessageRole::Assistant,
                            "I'll analyze this code for best practices, potential bugs, and improvements.",
                        ),
                    ],
                })
            }
            "debug_helper" => Ok(GetPromptResult {
                description: Some("Interactive debugging assistant".to_string()),
                messages: vec![
                    PromptMessage::new_text(
                        PromptMessageRole::Assistant,
                        "You are a helpful debugging assistant. Ask the user about their error and help them solve it.",
                    ),
                    PromptMessage::new_text(
                        PromptMessageRole::User,
                        "I need help debugging an issue in my code.",
                    ),
                    PromptMessage::new_text(
                        PromptMessageRole::Assistant,
                        "I'd be happy to help you debug your code! Please tell me:\n1. What error or issue are you experiencing?\n2. What programming language are you using?\n3. What were you trying to accomplish?",
                    ),
                ],
            }),
            _ => Err(McpError::invalid_params(
                format!("Unknown prompt: {}", name),
                Some(serde_json::json!({
                    "available_prompts": ["code_review", "debug_helper"]
                })),
            )),
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize the tracing subscriber
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive(tracing::Level::INFO.into()))
        .with_writer(std::io::stderr)
        .init();

    tracing::info!("Starting Prompt Example MCP server");

    // Create and serve the prompt server
    let service = PromptExampleServer.serve(stdio()).await?;

    service.waiting().await?;
    Ok(())
}
