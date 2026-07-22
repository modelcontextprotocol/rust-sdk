#![cfg(all(feature = "server", feature = "client", not(feature = "local")))]
#![allow(deprecated)]

use std::sync::{Arc, Mutex};

use rmcp::{
    ClientHandler, RoleClient, RoleServer, ServerHandler, ServiceError, ServiceExt,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ClientInfo, ContentBlock,
        CreateMessageRequest, CreateMessageRequestParams, CreateMessageResult, ProtocolVersion,
        SamplingMessage, ServerCapabilities, ServerInfo, ServerRequest,
    },
    service::RequestContext,
};
use tokio::sync::oneshot;

#[derive(Clone)]
struct SamplingServer {
    outside: Arc<Mutex<Option<oneshot::Sender<Result<(), ServiceError>>>>>,
}

impl ServerHandler for SamplingServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, rmcp::ErrorData> {
        let peer = context.peer.clone();
        let slot = self.outside.clone();

        let use_generic = request.name == "sample_generic";
        tokio::spawn(async move {
            let outside = if use_generic {
                peer.send_request(ServerRequest::CreateMessageRequest(
                    CreateMessageRequest::new(CreateMessageRequestParams::new(
                        vec![SamplingMessage::user_text("standalone-generic")],
                        16,
                    )),
                ))
                .await
                .map(|_| ())
            } else {
                peer.create_message(CreateMessageRequestParams::new(
                    vec![SamplingMessage::user_text("standalone")],
                    16,
                ))
                .await
                .map(|_| ())
            };
            if let Some(tx) = slot.lock().unwrap().take() {
                let _ = tx.send(outside);
            }
        });

        let nested = context
            .peer
            .create_message(CreateMessageRequestParams::new(
                vec![SamplingMessage::user_text("nested")],
                16,
            ))
            .await;
        nested.map_err(|e| rmcp::ErrorData::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text("ok")]).into())
    }
}

#[derive(Clone)]
struct SamplingClient;

impl ClientHandler for SamplingClient {
    async fn create_message(
        &self,
        _params: CreateMessageRequestParams,
        _context: RequestContext<RoleClient>,
    ) -> Result<CreateMessageResult, rmcp::ErrorData> {
        Ok(CreateMessageResult::new(
            SamplingMessage::assistant_text("pong"),
            "test-model".to_string(),
        )
        .with_stop_reason(CreateMessageResult::STOP_REASON_END_TURN))
    }

    fn get_info(&self) -> ClientInfo {
        let mut info = ClientInfo::default();
        info.protocol_version = ProtocolVersion::V_2026_07_28;
        info
    }
}

#[tokio::test]
async fn nested_sampling_allowed_standalone_rejected() -> anyhow::Result<()> {
    let (server_transport, client_transport) = tokio::io::duplex(4096);
    let (tx, rx) = oneshot::channel();
    let server = SamplingServer {
        outside: Arc::new(Mutex::new(Some(tx))),
    };
    let server_handle = tokio::spawn(async move {
        let running = server.serve(server_transport).await?;
        running.waiting().await?;
        anyhow::Ok(())
    });

    let client = SamplingClient.serve(client_transport).await?;

    let result = client
        .peer()
        .call_tool(CallToolRequestParams::new("sample"))
        .await?;
    assert_eq!(
        result.content.first().unwrap().as_text().unwrap().text,
        "ok"
    );

    let outside = rx.await?;
    assert!(matches!(outside, Err(ServiceError::McpError(_))));

    client.cancel().await?;
    let _ = server_handle.await?;
    Ok(())
}

#[tokio::test]
async fn generic_send_request_bypass_rejected() -> anyhow::Result<()> {
    let (server_transport, client_transport) = tokio::io::duplex(4096);
    let (tx, rx) = oneshot::channel();
    let server = SamplingServer {
        outside: Arc::new(Mutex::new(Some(tx))),
    };
    let server_handle = tokio::spawn(async move {
        let running = server.serve(server_transport).await?;
        running.waiting().await?;
        anyhow::Ok(())
    });

    let client = SamplingClient.serve(client_transport).await?;

    let result = client
        .peer()
        .call_tool(CallToolRequestParams::new("sample_generic"))
        .await?;
    assert_eq!(
        result.content.first().unwrap().as_text().unwrap().text,
        "ok"
    );

    let outside = rx.await?;
    assert!(
        matches!(outside, Err(ServiceError::McpError(_))),
        "generic send_request must not bypass SEP-2260 enforcement"
    );

    client.cancel().await?;
    let _ = server_handle.await?;
    Ok(())
}
