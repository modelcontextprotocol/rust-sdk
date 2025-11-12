//! Simple MCP Server with Elicitation
//!
//! Demonstrates user name collection via elicitation

use std::{
    fmt::{Display, Formatter},
    sync::Arc,
};

use anyhow::Result;
use rmcp::{
    ErrorData as McpError, ServerHandler, ServiceExt, elicit_safe,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::*,
    schemars::JsonSchema,
    service::{RequestContext, RoleServer},
    tool, tool_handler, tool_router,
    transport::stdio,
};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tracing_subscriber::{self, EnvFilter};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub enum UserType {
    #[schemars(title = "Guest User")]
    #[default]
    Guest,
    #[schemars(title = "Admin User")]
    Admin,
}

impl Display for UserType {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            UserType::Guest => write!(f, "Guest"),
            UserType::Admin => write!(f, "Admin"),
        }
    }
}

/// User information request
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "User information")]
pub struct UserInfo {
    #[schemars(description = "User's name")]
    pub name: String,
    #[schemars(title = "What kind of user you are?", default)]
    pub user_type: UserType,
}

// Mark as safe for elicitation
elicit_safe!(UserInfo);

/// Simple greeting message
#[derive(Debug, Serialize, Deserialize)]
pub struct GreetingMessage {
    pub text: String,
}

/// Simple tool request
#[derive(Debug, Deserialize, JsonSchema)]
pub struct GreetRequest {
    pub greeting: String,
}

/// Simple server with elicitation
#[derive(Clone)]
pub struct ElicitationServer {
    user_info: Arc<Mutex<Option<UserInfo>>>,
    tool_router: ToolRouter<ElicitationServer>,
}

impl ElicitationServer {
    pub fn new() -> Self {
        Self {
            user_info: Arc::new(Mutex::new(None)),
            tool_router: Self::tool_router(),
        }
    }
}

impl Default for ElicitationServer {
    fn default() -> Self {
        Self::new()
    }
}

#[tool_router]
impl ElicitationServer {
    #[tool(description = "Greet user with name collection")]
    async fn greet_user(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(request): Parameters<GreetRequest>,
    ) -> Result<CallToolResult, McpError> {
        // Check if we have user info
        let current_info = self.user_info.lock().await.clone();

        let user_info = if let Some(info) = current_info {
            info
        } else {
            // Request user name via elicitation
            match context
                .peer
                .elicit::<UserInfo>("Please provide your name".to_string())
                .await
            {
                Ok(Some(user_info)) => {
                    *self.user_info.lock().await = Some(user_info.clone());
                    user_info
                }
                Ok(None) => UserInfo {
                    name: "Guest".to_string(),
                    user_type: UserType::Guest,
                }, // Never happen if client checks schema
                Err(err) => {
                    tracing::error!("Failed to elicit user info: {:?}", err);
                    UserInfo {
                        name: "Unknown".to_string(),
                        user_type: UserType::Guest,
                    }
                }
            }
        };

        Ok(CallToolResult::success(vec![Content::text(format!(
            "{} {}! You are {}",
            request.greeting, user_info.name, user_info.user_type
        ))]))
    }

    #[tool(description = "Reset stored user name")]
    async fn reset_name(&self) -> Result<CallToolResult, McpError> {
        *self.user_info.lock().await = None;
        Ok(CallToolResult::success(vec![Content::text(
            "User name reset. Next greeting will ask for name again.".to_string(),
        )]))
    }
}

#[tool_handler]
impl ServerHandler for ElicitationServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation::from_build_env(),
            instructions: Some(
                "Simple server demonstrating elicitation for user name collection".to_string(),
            ),
            ..Default::default()
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    eprintln!("Simple MCP Elicitation Demo");

    // Get current executable path for Inspector
    let current_exe = std::env::current_exe()
        .map(|path| path.display().to_string())
        .unwrap();

    eprintln!("To test with MCP Inspector:");
    eprintln!("1. Run: npx @modelcontextprotocol/inspector");
    eprintln!("2. Enter server command: {}", current_exe);

    let service = ElicitationServer::new()
        .serve(stdio())
        .await
        .inspect_err(|e| {
            tracing::error!("serving error: {:?}", e);
        })?;

    service.waiting().await?;
    Ok(())
}
