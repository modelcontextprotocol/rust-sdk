use anyhow::Result;
use rmcp::{ServiceExt, transport::stdio, ErrorData as McpError, RoleServer, ServerHandler};
use rmcp::handler::server::{
    router::tool::ToolRouter,
    wrapper::Parameters,
};
use rmcp::model::*;
use rmcp::{tool, tool_handler, tool_router};
use std::fs;
use std::path::Path;
use std::process::Command;
use tracing_subscriber::{self, EnvFilter};

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ExecuteCommandArgs {
    /// The command to execute
    pub command: String,
    /// The working directory for the command (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working_directory: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ReadFileArgs {
    /// Path to the file to read
    pub path: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct WriteFileArgs {
    /// Path to the file to write
    pub path: String,
    /// Content to write to the file
    pub content: String,
}

#[derive(Clone)]
pub struct FileOperations {
    tool_router: ToolRouter<FileOperations>,
}

#[tool_router]
impl FileOperations {
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }

    #[tool(description = "Execute a shell command with optional working directory")]
    async fn execute_command(
        &self,
        Parameters(args): Parameters<ExecuteCommandArgs>,
    ) -> Result<CallToolResult, McpError> {
        let mut cmd = if cfg!(target_os = "windows") {
            let mut c = Command::new("cmd");
            c.args(["/C", &args.command]);
            c
        } else {
            let mut c = Command::new("sh");
            c.args(["-c", &args.command]);
            c
        };

        if let Some(working_dir) = args.working_directory {
            cmd.current_dir(working_dir);
        }

        match cmd.output() {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);
                let success = output.status.success();

                let result = format!(
                    "Exit code: {}\nStdout:\n{}\nStderr:\n{}",
                    output.status.code().unwrap_or(-1),
                    stdout,
                    stderr
                );

                Ok(CallToolResult {
                    content: vec![Content::text(result)],
                    structured_content: None,
                    is_error: Some(!success),
                    meta: None,
                })
            }
            Err(e) => Ok(CallToolResult {
                content: vec![Content::text(format!("Failed to execute command: {}", e))],
                structured_content: None,
                is_error: Some(true),
                meta: None,
            }),
        }
    }

    #[tool(description = "Read the contents of a file")]
    async fn read_file(
        &self,
        Parameters(args): Parameters<ReadFileArgs>,
    ) -> Result<CallToolResult, McpError> {
        match fs::read_to_string(&args.path) {
            Ok(content) => Ok(CallToolResult::success(vec![Content::text(content)])),
            Err(e) => Ok(CallToolResult {
                content: vec![Content::text(format!("Failed to read file '{}': {}", args.path, e))],
                structured_content: None,
                is_error: Some(true),
                meta: None,
            }),
        }
    }

    #[tool(description = "Write content to a file")]
    async fn write_file(
        &self,
        Parameters(args): Parameters<WriteFileArgs>,
    ) -> Result<CallToolResult, McpError> {
        // Create parent directories if they don't exist
        if let Some(parent) = Path::new(&args.path).parent() {
            if let Err(e) = fs::create_dir_all(parent) {
                return Ok(CallToolResult {
                    content: vec![Content::text(format!("Failed to create directories: {}", e))],
                    structured_content: None,
                    is_error: Some(true),
                    meta: None,
                });
            }
        }

        match fs::write(&args.path, &args.content) {
            Ok(_) => Ok(CallToolResult::success(vec![Content::text(format!(
                "Successfully wrote {} bytes to '{}'",
                args.content.len(),
                args.path
            ))])),
            Err(e) => Ok(CallToolResult {
                content: vec![Content::text(format!("Failed to write file '{}': {}", args.path, e))],
                structured_content: None,
                is_error: Some(true),
                meta: None,
            }),
        }
    }
}

#[tool_handler]
impl ServerHandler for FileOperations {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::V_2024_11_05,
            capabilities: ServerCapabilities::builder()
                .enable_tools()
                .build(),
            server_info: Implementation::from_build_env(),
            instructions: Some("This server provides file and command execution tools for AI CLI integration. Available tools: execute_command (run shell commands), read_file (read file contents), write_file (write content to files).".to_string()),
        }
    }

    async fn initialize(
        &self,
        _request: InitializeRequestParam,
        _context: rmcp::service::RequestContext<RoleServer>,
    ) -> Result<InitializeResult, McpError> {
        Ok(self.get_info())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging to stderr to avoid interfering with MCP protocol on stdout
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    tracing::info!("Starting File Operations MCP server");

    // Create the file operations service
    let service = FileOperations::new()
        .serve(stdio())
        .await
        .inspect_err(|e| {
            tracing::error!("serving error: {:?}", e);
        })?;

    // Wait for the service to complete
    service.waiting().await?;
    Ok(())
}