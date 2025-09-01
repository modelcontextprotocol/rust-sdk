use anyhow::Result;
use rmcp::{
    ClientHandler, ServiceExt,
    model::*,
    object,
    service::{RequestContext, RoleClient},
    transport::{ConfigureCommandExt, TokioChildProcess},
};
use tokio::process::Command;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
/// Simple Sampling Demo Client
///
/// This client demonstrates how to handle sampling requests from servers.
/// It includes a mock LLM that generates simple responses.
/// Run with: cargo run --example clients_sampling_stdio
#[derive(Clone, Debug, Default)]
pub struct SamplingDemoClient;

impl SamplingDemoClient {
    /// Mock LLM function that generates responses based on the input
    /// In actual implementation, this would be replaced with a call to an LLM service
    fn mock_llm_response(
        &self,
        _messages: &[SamplingMessage],
        _system_prompt: Option<&str>,
    ) -> String {
        "It just a mock response".to_string()
    }
}

impl ClientHandler for SamplingDemoClient {
    async fn create_message(
        &self,
        params: CreateMessageRequestParam,
        _context: RequestContext<RoleClient>,
    ) -> Result<CreateMessageResult, ErrorData> {
        tracing::info!("Received sampling request with {:?}", params);

        // Generate mock response using our simple LLM
        let response_text =
            self.mock_llm_response(&params.messages, params.system_prompt.as_deref());

        Ok(CreateMessageResult {
            message: SamplingMessage {
                role: Role::Assistant,
                content: Content::text(response_text),
            },
            model: "mock_llm".to_string(),
            stop_reason: Some(CreateMessageResult::STOP_REASON_END_TURN.to_string()),
        })
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| format!("info,{}=debug", env!("CARGO_CRATE_NAME")).into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    tracing::info!("Starting Sampling Demo Client");

    let client = SamplingDemoClient;

    // Start the sampling server as a child process
    let servers_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("CARGO_MANIFEST_DIR is not set")
        .join("servers");

    let client = client
        .serve(TokioChildProcess::new(Command::new("cargo").configure(
            |cmd| {
                cmd.arg("run")
                    .arg("--example")
                    .arg("servers_sampling_stdio")
                    .current_dir(servers_dir);
            },
        ))?)
        .await
        .inspect_err(|e| {
            tracing::error!("client error: {:?}", e);
        })?;

    // Wait for initialization
    tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;

    // Get server info
    let server_info = client.peer_info();
    tracing::info!("Connected to server: {server_info:#?}");

    // List available tools
    match client.list_all_tools().await {
        Ok(tools) => {
            tracing::info!("Available tools: {tools:#?}");

            // Test the ask_llm tool
            tracing::info!("Testing ask_llm tool...");
            match client
                .call_tool(CallToolRequestParam {
                    name: "ask_llm".into(),
                    arguments: Some(object!({
                        "question": "Hello world"
                    })),
                })
                .await
            {
                Ok(result) => tracing::info!("Ask LLM result: {result:#?}"),
                Err(e) => tracing::error!("Ask LLM error: {e}"),
            }
        }
        Err(e) => tracing::error!("Failed to list tools: {e}"),
    }

    tracing::info!("Sampling demo completed successfully!");

    tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
    client.cancel().await?;
    Ok(())
}
