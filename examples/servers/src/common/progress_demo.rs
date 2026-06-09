use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
};

use futures::Stream;
use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler, model::*, service::RequestContext, tool,
    tool_handler, tool_router,
};
use serde_json::json;
use tokio_stream::StreamExt;
use tracing::{debug, info};

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
        Self::new(text.as_bytes().to_vec(), 5)
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
}

#[tool_router]
impl ProgressDemo {
    #[allow(dead_code)]
    pub fn new() -> Self {
        Self {
            data_source: StreamDataSource::from_text("1111122222333334444455555"),
        }
    }
    #[tool(description = "Process data stream with progress updates")]
    async fn stream_processor(
        &self,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let mut counter = 0;
        info!(
            "Processing stream with progress token {:?}",
            ctx.meta.get_key_value("progressToken")
        );
        let Some((_, progress_token)) = ctx.meta.get_key_value("progressToken") else {
            return Err(McpError::internal_error(format!("No progress token"), None));
        };

        let Ok(progress_token) = serde_json::from_value::<NumberOrString>(progress_token.clone())
        else {
            return Err(McpError::internal_error(
                format!("Invalid format of the progress token"),
                None,
            ));
        };

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
                progress_token: ProgressToken(progress_token.clone()),
                progress: counter as f64,
                total: Some(5.0),
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
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
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
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_protocol_version(ProtocolVersion::V_2024_11_05)
            .with_server_info(Implementation::from_build_env())
            .with_instructions(
                "This server demonstrates progress notifications during long-running operations. \
                 Use the tools to see real-time progress updates for batch processing"
                    .to_string(),
            )
    }
}
