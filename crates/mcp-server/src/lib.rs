use futures::Future;
use mcp_core::protocol::{JsonRpcError, JsonRpcMessage, JsonRpcRequest, JsonRpcResponse};
use std::pin::Pin;
use tower_service::Service;

mod errors;
pub use errors::{BoxError, RouterError, ServerError, TransportError};

pub mod router;
pub use router::Router;
use transport::Transport;

pub mod toolset;
pub mod transport;

/// The main server type that processes incoming requests
pub struct Server<S> {
    service: S,
}

impl<S> Server<S>
where
    S: Service<JsonRpcRequest, Response = JsonRpcResponse> + Send,
    S::Error: Into<BoxError>,
    S::Future: Send,
{
    pub fn new(service: S) -> Self {
        Self { service }
    }

    pub async fn run<T>(self, mut transport: T) -> Result<(), ServerError>
    where
        T: Transport + Unpin,
    {
        use futures::StreamExt;
        let mut service = self.service;

        tracing::info!("Server started");
        while let Some(msg_result) = transport.next().await {
            let _span = tracing::span!(tracing::Level::INFO, "message_processing");
            let _enter = _span.enter();
            match msg_result {
                Ok(msg) => {
                    match msg {
                        JsonRpcMessage::Request(request) => {
                            // Serialize request for logging
                            let id = request.id;
                            let request_json = serde_json::to_string(&request)
                                .unwrap_or_else(|_| "Failed to serialize request".to_string());

                            tracing::info!(
                                request_id = ?id,
                                method = ?request.method,
                                json = %request_json,
                                "Received request"
                            );

                            // Process the request using our service
                            let response = match service.call(request).await {
                                Ok(resp) => resp,
                                Err(e) => {
                                    let error_msg = e.into().to_string();
                                    tracing::error!(error = %error_msg, "Request processing failed");
                                    JsonRpcResponse {
                                        jsonrpc: "2.0".to_string(),
                                        id,
                                        result: None,
                                        error: Some(mcp_core::protocol::ErrorData {
                                            code: mcp_core::protocol::INTERNAL_ERROR,
                                            message: error_msg,
                                            data: None,
                                        }),
                                    }
                                }
                            };

                            // Serialize response for logging
                            let response_json = serde_json::to_string(&response)
                                .unwrap_or_else(|_| "Failed to serialize response".to_string());

                            tracing::info!(
                                response_id = ?response.id,
                                json = %response_json,
                                "Sending response"
                            );
                            // Send the response back
                            if let Err(e) = transport
                                .write_message(JsonRpcMessage::Response(response))
                                .await
                            {
                                return Err(ServerError::Transport(e));
                            }
                        }
                        JsonRpcMessage::Response(_)
                        | JsonRpcMessage::Notification(_)
                        | JsonRpcMessage::Nil
                        | JsonRpcMessage::Error(_) => {
                            // Ignore responses, notifications and nil messages for now
                            continue;
                        }
                    }
                }
                Err(e) => {
                    // Convert transport error to JSON-RPC error response
                    let error = match e {
                        TransportError::Json(_) | TransportError::InvalidMessage(_) => {
                            mcp_core::protocol::ErrorData {
                                code: mcp_core::protocol::PARSE_ERROR,
                                message: e.to_string(),
                                data: None,
                            }
                        }
                        TransportError::Protocol(_) => mcp_core::protocol::ErrorData {
                            code: mcp_core::protocol::INVALID_REQUEST,
                            message: e.to_string(),
                            data: None,
                        },
                        _ => mcp_core::protocol::ErrorData {
                            code: mcp_core::protocol::INTERNAL_ERROR,
                            message: e.to_string(),
                            data: None,
                        },
                    };

                    let error_response = JsonRpcMessage::Error(JsonRpcError {
                        jsonrpc: "2.0".to_string(),
                        id: None,
                        error,
                    });

                    if let Err(e) = transport.write_message(error_response).await {
                        return Err(ServerError::Transport(e));
                    }
                }
            }
        }

        Ok(())
    }
}

// Define a specific service implementation that we need for any
// Any router implements this
pub trait BoundedService:
    Service<
        JsonRpcRequest,
        Response = JsonRpcResponse,
        Error = BoxError,
        Future = Pin<Box<dyn Future<Output = Result<JsonRpcResponse, BoxError>> + Send>>,
    > + Send
    + 'static
{
}

// Implement it for any type that meets the bounds
impl<T> BoundedService for T where
    T: Service<
            JsonRpcRequest,
            Response = JsonRpcResponse,
            Error = BoxError,
            Future = Pin<Box<dyn Future<Output = Result<JsonRpcResponse, BoxError>> + Send>>,
        > + Send
        + 'static
{
}
