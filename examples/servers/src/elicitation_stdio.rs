//! Simple MCP Server with Elicitation
//!
//! Demonstrates user name collection via elicitation

use std::sync::Arc;

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

/// User information request
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "User information")]
pub struct UserInfo {
    #[schemars(description = "User's name")]
    pub name: String,
}

// Mark as safe for elicitation
elicit_safe!(UserInfo);

/// Simple tool request
#[derive(Debug, Deserialize, JsonSchema)]
pub struct GreetRequest {
    pub greeting: String,
}

/// Simple server with elicitation
#[derive(Clone)]
pub struct ElicitationServer {
    user_name: Arc<Mutex<Option<String>>>,
    tool_router: ToolRouter<ElicitationServer>,
}

impl ElicitationServer {
    pub fn new() -> Self {
        Self {
            user_name: Arc::new(Mutex::new(None)),
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
        // Check if we have user name
        let current_name = self.user_name.lock().await.clone();

        let user_name = if let Some(name) = current_name {
            name
        } else {
            // Request user name via elicitation
            match context
                .peer
                .elicit::<UserInfo>("Please provide your name".to_string())
                .await
            {
                Ok(Some(user_info)) => {
                    let name = user_info.name.clone();
                    *self.user_name.lock().await = Some(name.clone());
                    name
                }
                Ok(None) => "Guest".to_string(), // Never happen if client checks schema
                Err(_) => "Unknown".to_string(),
            }
        };

        Ok(CallToolResult::success(vec![Content::text(format!(
            "{} {}!",
            request.greeting, user_name
        ))]))
    }

    #[tool(description = "Reset stored user name")]
    async fn reset_name(&self) -> Result<CallToolResult, McpError> {
        *self.user_name.lock().await = None;
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

    println!("Simple MCP Elicitation Demo");

    // Get current executable path for Inspector
    let current_exe = std::env::current_exe()
        .map(|path| path.display().to_string())
        .unwrap();

    println!("To test with MCP Inspector:");
    println!("1. Run: npx @modelcontextprotocol/inspector");
    println!("2. Enter server command: {}", current_exe);

    let service = ElicitationServer::new()
        .serve(stdio())
        .await
        .inspect_err(|e| {
            tracing::error!("serving error: {:?}", e);
        })?;

    service.waiting().await?;
    Ok(())
}
