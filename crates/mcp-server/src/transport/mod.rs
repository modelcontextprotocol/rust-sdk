use async_trait::async_trait;
use futures::Stream;
use mcp_core::protocol::JsonRpcMessage;

use crate::TransportError;

pub mod stdio;
pub use stdio::StdioTransport;

/// A trait representing a transport layer for JSON-RPC messages
#[async_trait]
pub trait Transport: Stream<Item = Result<JsonRpcMessage, TransportError>> {
    /// Writes a JSON-RPC message to the transport
    async fn write_message(&mut self, message: JsonRpcMessage) -> Result<(), TransportError>;
}
