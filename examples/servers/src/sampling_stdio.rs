use std::sync::Arc;

use anyhow::Result;
use rmcp::{
    ServerHandler, ServiceExt,
    model::*,
    service::{RequestContext, RoleServer},
    transport::stdio,
};
use tracing_subscriber::{self, EnvFilter};

/// Simple Sampling Demo Server
///
/// This server demonstrates how to request LLM sampling from clients.
/// Run with: cargo run --example servers_sampling_stdio
#[derive(Clone, Debug, Default)]
pub struct SamplingDemoServer;

impl ServerHandler for SamplingDemoServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some(concat!(
                "This is a demo server that requests sampling from clients. It provides tools that use LLM capabilities.\n\n",
                "IMPORTANT: This server requires a client that supports the 'sampling/createMessage' method. ",
                "Without sampling support, the tools will return errors."
            ).into()),
            capabilities: ServerCapabilities::builder()
                .enable_tools()
                .build(),
            ..Default::default()
        }
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParam,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        match request.name.as_ref() {
            "ask_llm" => {
                // Get the question from arguments
                let question = request
                    .arguments
                    .as_ref()
                    .and_then(|args| args.get("question"))
                    .and_then(|q| q.as_str())
                    .unwrap_or("What is the capital of France?");

                // Request sampling from the client
                let sampling_request = CreateMessageRequest {
                    method: Default::default(),
                    params: CreateMessageRequestParam {
                        messages: vec![SamplingMessage {
                            role: Role::User,
                            content: Content::text(question),
                        }],
                        model_preferences: Some(ModelPreferences {
                            hints: Some(vec![ModelHint {
                                name: Some("claude".to_string()),
                            }]),
                            cost_priority: Some(0.3),
                            speed_priority: Some(0.8),
                            intelligence_priority: Some(0.7),
                        }),
                        system_prompt: Some("You are a helpful assistant.".to_string()),
                        temperature: Some(0.7),
                        max_tokens: 150,
                        stop_sequences: None,
                        include_context: Some(ContextInclusion::None),
                        metadata: None,
                    },
                    extensions: Default::default(),
                };
                let request = ServerRequest::CreateMessageRequest(sampling_request.clone());
                tracing::info!("Sending request: {:?}", request);
                let response = context.peer.send_request(request).await.map_err(|e| {
                    ErrorData::new(
                        ErrorCode::INTERNAL_ERROR,
                        format!("Sampling request failed: {}", e),
                        None,
                    )
                })?;
                if let ClientResult::CreateMessageResult(result) = response {
                    Ok(CallToolResult::success(vec![Content::text(format!(
                        "Question: {}\nAnswer: {}",
                        question,
                        result
                            .message
                            .content
                            .as_text()
                            .map(|t| &t.text)
                            .unwrap_or(&"No text response".to_string())
                    ))]))
                } else {
                    Err(ErrorData::new(
                        ErrorCode::INTERNAL_ERROR,
                        "Unexpected response type",
                        None,
                    ))
                }
            }

            _ => Err(ErrorData::new(
                ErrorCode::INTERNAL_ERROR,
                format!("Unknown tool: {}", request.name),
                None,
            )),
        }
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParam>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult {
            tools: vec![Tool {
                name: "ask_llm".into(),
                description: Some("Ask a question to the LLM through sampling".into()),
                input_schema: Arc::new(
                    serde_json::from_value(serde_json::json!({
                        "type": "object",
                        "properties": {
                            "question": {
                                "type": "string",
                                "description": "The question to ask the LLM"
                            }
                        },
                        "required": ["question"]
                    }))
                    .unwrap(),
                ),
                annotations: None,
            }],
            next_cursor: None,
        })
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive(tracing::Level::INFO.into()))
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    tracing::info!("Starting Sampling Demo Server");

    // Create and serve the sampling demo server
    let service = SamplingDemoServer.serve(stdio()).await.inspect_err(|e| {
        tracing::error!("Serving error: {:?}", e);
    })?;

    service.waiting().await?;
    Ok(())
}
