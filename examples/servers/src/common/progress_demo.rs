use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
};

use futures::Stream;
use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler, handler::server::tool::ToolRouter, model::*,
    service::RequestContext, tool, tool_handler, tool_router,
};
use serde_json::json;
use tokio_stream::StreamExt;
use tracing::debug;

// a Stream data source that generates data in chunks
#[derive(Clone)]
struct StreamDataSource {
    data: Vec<u8>,
    chunk_size: usize,
    position: usize,
}

impl StreamDataSource {
    pub fn new(data: Vec<u8>, chunk_size: usize) -> Self {
        Self {
            data,
            chunk_size,
            position: 0,
        }
    }
    pub fn from_text(text: &str) -> Self {
        Self::new(text.as_bytes().to_vec(), 1)
    }
}

impl Stream for StreamDataSource {
    type Item = Result<Vec<u8>, io::Error>;

    fn poll_next(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if this.position >= this.data.len() {
            return Poll::Ready(None);
        }

        let start = this.position;
        let end = (start + this.chunk_size).min(this.data.len());
        let chunk = this.data[start..end].to_vec();
        this.position = end;
        Poll::Ready(Some(Ok(chunk)))
    }
}

#[derive(Clone)]
pub struct ProgressDemo {
    data_source: StreamDataSource,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl ProgressDemo {
    #[allow(dead_code)]
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
            data_source: StreamDataSource::from_text("Hello, world!"),
        }
    }
    #[tool(description = "Process data stream with progress updates")]
    async fn stream_processor(
        &self,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let mut counter = 0;

        let mut data_source = self.data_source.clone();
        loop {
            let chunk = data_source.next().await;
            if chunk.is_none() {
                break;
            }

            let chunk = chunk.unwrap().unwrap();
            let chunk_str = String::from_utf8_lossy(&chunk);
            counter += 1;
            // create progress notification param
            let progress_param = ProgressNotificationParam {
                progress_token: ProgressToken(NumberOrString::Number(counter)),
                progress: counter as f64,
                total: None,
                message: Some(chunk_str.to_string()),
            };

            match ctx.peer.notify_progress(progress_param).await {
                Ok(_) => {
                    debug!("Processed record: {}", chunk_str);
                }
                Err(e) => {
                    return Err(McpError::internal_error(
                        format!("Failed to notify progress: {}", e),
                        Some(json!({
                            "record": chunk_str,
                            "progress": counter,
                            "error": e.to_string()
                        })),
                    ));
                }
            }
        }

        Ok(CallToolResult::success(vec![Content::text(format!(
            "Processed {} records successfully",
            counter
        ))]))
    }
}

#[tool_handler]
impl ServerHandler for ProgressDemo {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::V_2024_11_05,
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation::from_build_env(),
            instructions: Some(
                "This server demonstrates progress notifications during long-running operations. \
                 Use the tools to see real-time progress updates for batch processing"
                    .to_string(),
            ),
        }
    }
}
