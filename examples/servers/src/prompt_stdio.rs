//! MCP Server demonstrating prompt functionality
//!
//! This example shows how to use prompt macros to create an MCP server
//! that provides various types of prompts to LLM clients.
//!
//! Run with MCP Inspector:
//! ```bash
//! npx @modelcontextprotocol/inspector cargo run -p mcp-server-examples --example servers_prompt_stdio
//! ```

use std::sync::Arc;

use anyhow::Result;
use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler, ServiceExt,
    handler::server::{router::prompt::PromptRouter, wrapper::Parameters},
    model::*,
    prompt, prompt_handler, prompt_router,
    schemars::JsonSchema,
    service::RequestContext,
    transport::stdio,
};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing_subscriber::{self, EnvFilter};

/// Arguments for the code review prompt
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "Code review parameters")]
pub struct CodeReviewArgs {
    #[schemars(description = "Programming language of the code")]
    pub language: String,
    #[schemars(description = "Path to the file or code snippet")]
    pub file_path: String,
    #[schemars(description = "Focus areas for the review")]
    pub focus_areas: Option<Vec<String>>,
}

/// Arguments for the data analysis prompt
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "Data analysis parameters")]
pub struct DataAnalysisArgs {
    #[schemars(description = "Type of data: 'csv', 'json', 'logs', etc.")]
    pub data_type: String,
    #[schemars(description = "What kind of analysis to perform")]
    pub analysis_type: String,
    #[schemars(description = "Additional context about the data")]
    pub context: Option<String>,
}

/// Arguments for the writing assistant prompt
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "Writing assistant parameters")]
pub struct WritingAssistantArgs {
    #[schemars(description = "Type of content: 'email', 'documentation', 'blog', etc.")]
    pub content_type: String,
    #[schemars(description = "Target audience")]
    pub audience: String,
    #[schemars(description = "Writing tone: 'formal', 'casual', 'technical', etc.")]
    pub tone: Option<String>,
    #[schemars(description = "Key points to cover")]
    pub key_points: Vec<String>,
}

/// Arguments for the debug assistant prompt
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "Debug assistant parameters")]
pub struct DebugAssistantArgs {
    #[schemars(description = "Error message or symptom")]
    pub error_message: String,
    #[schemars(description = "Technology stack involved")]
    pub stack: Vec<String>,
    #[schemars(description = "Steps already tried")]
    pub tried_solutions: Option<Vec<String>>,
}

/// Simple prompt server demonstrating various prompt patterns
#[derive(Clone)]
pub struct PromptServer {
    /// Stores user preferences that can be used in prompts
    user_preferences: Arc<RwLock<UserPreferences>>,
    prompt_router: PromptRouter<PromptServer>,
}

#[derive(Debug, Clone)]
struct UserPreferences {
    preferred_language: String,
    expertise_level: String,
}

impl Default for UserPreferences {
    fn default() -> Self {
        Self {
            preferred_language: "English".to_string(),
            expertise_level: "intermediate".to_string(),
        }
    }
}

impl PromptServer {
    pub fn new() -> Self {
        Self {
            user_preferences: Arc::new(RwLock::new(UserPreferences::default())),
            prompt_router: Self::prompt_router(),
        }
    }
}

impl Default for PromptServer {
    fn default() -> Self {
        Self::new()
    }
}

#[prompt_router]
impl PromptServer {
    /// Simple greeting prompt without parameters
    #[prompt(
        name = "greeting",
        description = "A simple greeting prompt to start conversations"
    )]
    async fn greeting(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(
                PromptMessageRole::User,
                "Hello! I'd like to start our conversation.",
            ),
            PromptMessage::new_text(
                PromptMessageRole::Assistant,
                "Hello! I'm here to help. What would you like to discuss today?",
            ),
        ]
    }

    /// Code review prompt with typed parameters
    #[prompt(
        name = "code_review",
        description = "Structured code review with language-specific best practices"
    )]
    async fn code_review(
        &self,
        Parameters(args): Parameters<CodeReviewArgs>,
    ) -> Result<GetPromptResult, McpError> {
        let prefs = self.user_preferences.read().await;
        let focus_areas = args
            .focus_areas
            .unwrap_or_else(|| vec!["correctness".to_string(), "performance".to_string()]);

        let messages = vec![
            PromptMessage::new_text(
                PromptMessageRole::Assistant,
                format!(
                    "You are an expert {} code reviewer. The user's expertise level is {}.",
                    args.language, prefs.expertise_level
                ),
            ),
            PromptMessage::new_text(
                PromptMessageRole::User,
                format!(
                    "Please review the {} code at '{}'. Focus on: {}",
                    args.language,
                    args.file_path,
                    focus_areas.join(", ")
                ),
            ),
            PromptMessage::new_text(
                PromptMessageRole::Assistant,
                format!(
                    "I'll review your {} code focusing on {}. Let me analyze the code at '{}'...",
                    args.language,
                    focus_areas.join(" and "),
                    args.file_path
                ),
            ),
        ];

        Ok(GetPromptResult {
            description: Some(format!(
                "Code review for {} file focusing on {}",
                args.language,
                focus_areas.join(", ")
            )),
            messages,
        })
    }

    /// Data analysis prompt demonstrating context usage
    #[prompt(
        name = "data_analysis",
        description = "Analyze data with context-aware suggestions"
    )]
    async fn data_analysis(
        &self,
        Parameters(args): Parameters<DataAnalysisArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<Vec<PromptMessage>, McpError> {
        // Log the request for debugging
        tracing::info!("Data analysis prompt requested for {} data", args.data_type);

        // Could use ctx to check for capabilities or metadata
        let _request_id = &ctx.id;

        let context = args
            .context
            .unwrap_or_else(|| "General analysis requested".to_string());

        Ok(vec![
            PromptMessage::new_text(
                PromptMessageRole::User,
                format!(
                    "I have {} data that needs {} analysis. Context: {}",
                    args.data_type, args.analysis_type, context
                ),
            ),
            PromptMessage::new_text(
                PromptMessageRole::Assistant,
                format!(
                    "I'll help you analyze your {} data using {} techniques. Based on your context, \
                     I'll focus on providing actionable insights.",
                    args.data_type, args.analysis_type
                ),
            ),
        ])
    }

    /// Writing assistant with multiple conversation turns
    #[prompt(
        name = "writing_assistant",
        description = "Multi-turn writing assistance with style guidance"
    )]
    async fn writing_assistant(
        &self,
        Parameters(args): Parameters<WritingAssistantArgs>,
    ) -> GetPromptResult {
        let tone = args.tone.unwrap_or_else(|| "professional".to_string());

        let mut messages = vec![
            PromptMessage::new_text(
                PromptMessageRole::Assistant,
                format!(
                    "You are a writing assistant helping create {} content for {}. \
                     Use a {} tone.",
                    args.content_type, args.audience, tone
                ),
            ),
            PromptMessage::new_text(
                PromptMessageRole::User,
                format!(
                    "I need help writing {} for {}. Key points to cover: {}",
                    args.content_type,
                    args.audience,
                    args.key_points.join(", ")
                ),
            ),
            PromptMessage::new_text(
                PromptMessageRole::Assistant,
                "I'll help you create that content. Let me structure it based on your key points.",
            ),
        ];

        // Add a message for each key point
        for (i, point) in args.key_points.iter().enumerate() {
            messages.push(PromptMessage::new_text(
                PromptMessageRole::User,
                format!("For point {}: {}, what would you suggest?", i + 1, point),
            ));
            messages.push(PromptMessage::new_text(
                PromptMessageRole::Assistant,
                format!("For '{}', I recommend...", point),
            ));
        }

        GetPromptResult {
            description: Some(format!(
                "Writing {} for {} audience with {} tone",
                args.content_type, args.audience, tone
            )),
            messages,
        }
    }

    /// Debug assistant demonstrating error handling patterns
    #[prompt(
        name = "debug_assistant",
        description = "Interactive debugging help with solution tracking"
    )]
    async fn debug_assistant(
        &self,
        Parameters(args): Parameters<DebugAssistantArgs>,
    ) -> Result<GetPromptResult, McpError> {
        if args.stack.is_empty() {
            return Err(McpError::invalid_params(
                "Technology stack cannot be empty",
                None,
            ));
        }

        let mut messages = vec![
            PromptMessage::new_text(
                PromptMessageRole::Assistant,
                format!(
                    "You are a debugging expert for {}. Help diagnose and fix issues.",
                    args.stack.join(", ")
                ),
            ),
            PromptMessage::new_text(
                PromptMessageRole::User,
                format!(
                    "I'm encountering this error: {}\nStack: {}",
                    args.error_message,
                    args.stack.join(", ")
                ),
            ),
        ];

        // Add tried solutions if any
        if let Some(tried) = args.tried_solutions {
            if !tried.is_empty() {
                messages.push(PromptMessage::new_text(
                    PromptMessageRole::User,
                    format!("I've already tried: {}", tried.join(", ")),
                ));
                messages.push(PromptMessage::new_text(
                    PromptMessageRole::Assistant,
                    "I see you've already attempted some solutions. Let me suggest different approaches.",
                ));
            }
        }

        messages.push(PromptMessage::new_text(
            PromptMessageRole::Assistant,
            "Let's debug this systematically. First, let me understand the error context better.",
        ));

        Ok(GetPromptResult {
            description: Some(format!(
                "Debugging {} error in {}",
                args.error_message.chars().take(50).collect::<String>(),
                args.stack.first().map(|s| s.as_str()).unwrap_or("unknown")
            )),
            messages,
        })
    }

    /// Learning path prompt that uses server state
    #[prompt(
        name = "learning_path",
        description = "Generate a personalized learning path based on user preferences"
    )]
    async fn learning_path(&self) -> Result<Vec<PromptMessage>, McpError> {
        let prefs = self.user_preferences.read().await;

        Ok(vec![
            PromptMessage::new_text(
                PromptMessageRole::Assistant,
                format!(
                    "Create a learning path for someone at {} level who prefers {} language explanations.",
                    prefs.expertise_level, prefs.preferred_language
                ),
            ),
            PromptMessage::new_text(
                PromptMessageRole::User,
                "What should I learn next to improve my programming skills?",
            ),
            PromptMessage::new_text(
                PromptMessageRole::Assistant,
                format!(
                    "Based on your {} expertise level, I recommend the following learning path...",
                    prefs.expertise_level
                ),
            ),
        ])
    }
}

#[prompt_handler]
impl ServerHandler for PromptServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            capabilities: ServerCapabilities::builder().enable_prompts().build(),
            server_info: Implementation::from_build_env(),
            instructions: Some(
                "This server provides various prompt templates for code review, data analysis, \
                 writing assistance, debugging help, and personalized learning paths. \
                 All prompts are designed to provide structured, context-aware assistance."
                    .to_string(),
            ),
            ..Default::default()
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive(tracing::Level::INFO.into()))
        .init();

    println!("MCP Prompt Server Example");
    println!("=======================");
    println!();
    println!("This server demonstrates various prompt patterns:");
    println!("- Simple prompts without parameters");
    println!("- Prompts with typed parameters");
    println!("- Prompts using server state");
    println!("- Multi-turn conversation prompts");
    println!("- Error handling in prompts");
    println!();
    println!("To test with MCP Inspector:");
    println!(
        "npx @modelcontextprotocol/inspector cargo run -p mcp-server-examples --example servers_prompt_stdio"
    );
    println!();

    let server = PromptServer::new();
    let service = server.serve(stdio()).await.inspect_err(|e| {
        tracing::error!("Server error: {:?}", e);
    })?;

    service.waiting().await?;
    Ok(())
}
