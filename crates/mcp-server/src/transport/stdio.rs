use std::{
    pin::Pin,
    task::{Context, Poll},
};

use async_trait::async_trait;
use futures::{Future, Stream};
use mcp_core::protocol::JsonRpcMessage;
use pin_project::pin_project;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

use super::Transport;
use crate::TransportError;

/// A transport layer that handles JSON-RPC messages over byte
#[pin_project]
pub struct StdioTransport<R, W> {
    // Reader is a BufReader on the underlying stream (stdin or similar) buffering
    // the underlying data across poll calls, we clear one line (\n) during each
    // iteration of poll_next from this buffer
    #[pin]
    reader: BufReader<R>,
    #[pin]
    writer: W,
}

impl<R, W> StdioTransport<R, W>
where
    R: AsyncRead,
    W: AsyncWrite,
{
    pub fn new(reader: R, writer: W) -> Self {
        Self {
            // Default BufReader capacity is 8 * 1024, increase this to 2MB to the file size limit
            // allows the buffer to have the capacity to read very large calls
            reader: BufReader::with_capacity(2 * 1024 * 1024, reader),
            writer,
        }
    }
}

impl<R, W> Stream for StdioTransport<R, W>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    type Item = Result<JsonRpcMessage, TransportError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();
        let mut buf = Vec::new();

        let mut reader = this.reader.as_mut();
        let mut read_future = Box::pin(reader.read_until(b'\n', &mut buf));
        match read_future.as_mut().poll(cx) {
            Poll::Ready(Ok(0)) => Poll::Ready(None), // EOF
            Poll::Ready(Ok(_)) => {
                // Convert to UTF-8 string
                let line = match String::from_utf8(buf) {
                    Ok(s) => s,
                    Err(e) => return Poll::Ready(Some(Err(TransportError::Utf8(e)))),
                };
                // Log incoming message here before serde conversion to
                // track incomplete chunks which are not valid JSON
                tracing::info!(json = %line, "incoming message");

                // Parse JSON and validate message format
                match serde_json::from_str::<serde_json::Value>(&line) {
                    Ok(value) => {
                        // Validate basic JSON-RPC structure
                        if !value.is_object() {
                            return Poll::Ready(Some(Err(TransportError::InvalidMessage(
                                "Message must be a JSON object".into(),
                            ))));
                        }
                        let obj = value.as_object().unwrap(); // Safe due to check above

                        // Check jsonrpc version field
                        if !obj.contains_key("jsonrpc") || obj["jsonrpc"] != "2.0" {
                            return Poll::Ready(Some(Err(TransportError::InvalidMessage(
                                "Missing or invalid jsonrpc version".into(),
                            ))));
                        }

                        // Now try to parse as proper message
                        match serde_json::from_value::<JsonRpcMessage>(value) {
                            Ok(msg) => Poll::Ready(Some(Ok(msg))),
                            Err(e) => Poll::Ready(Some(Err(TransportError::Json(e)))),
                        }
                    }
                    Err(e) => Poll::Ready(Some(Err(TransportError::Json(e)))),
                }
            }
            Poll::Ready(Err(e)) => Poll::Ready(Some(Err(TransportError::Io(e)))),
            Poll::Pending => Poll::Pending,
        }
    }
}

#[async_trait]
impl<R, W> Transport for StdioTransport<R, W>
where
    R: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send,
{
    async fn write_message(&mut self, message: JsonRpcMessage) -> Result<(), TransportError> {
        let json = serde_json::to_string(&message).map_err(|e| TransportError::Json(e))?;

        Pin::new(&mut self.writer)
            .write_all(json.as_bytes())
            .await
            .map_err(|e| TransportError::Io(e))?;

        Pin::new(&mut self.writer)
            .write_all(b"\n")
            .await
            .map_err(|e| TransportError::Io(e))?;

        Pin::new(&mut self.writer)
            .flush()
            .await
            .map_err(|e| TransportError::Io(e))?;

        Ok(())
    }
}
