#![cfg(not(feature = "local"))]

use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use rmcp::{
    ClientHandler, Peer, RoleServer, ServiceError, ServiceExt,
    model::{CallToolRequestParams, ClientRequest, Meta, ProgressNotificationParam, Request},
    service::PeerRequestOptions,
    tool, tool_router,
};

#[derive(Clone, Default)]
struct ProgressCountingClient {
    progress_count: Arc<AtomicUsize>,
}

impl ClientHandler for ProgressCountingClient {
    async fn on_progress(
        &self,
        _params: ProgressNotificationParam,
        _context: rmcp::service::NotificationContext<rmcp::RoleClient>,
    ) {
        self.progress_count.fetch_add(1, Ordering::SeqCst);
    }
}

struct ProgressTimeoutServer;

impl ProgressTimeoutServer {
    fn new() -> Self {
        Self
    }
}

#[tool_router(server_handler)]
impl ProgressTimeoutServer {
    #[tool]
    async fn delayed_without_progress(&self) -> Result<(), rmcp::ErrorData> {
        tokio::time::sleep(Duration::from_millis(250)).await;
        Ok(())
    }

    #[tool]
    async fn delayed_with_progress(
        &self,
        meta: Meta,
        client: Peer<RoleServer>,
    ) -> Result<(), rmcp::ErrorData> {
        let progress_token = meta
            .get_progress_token()
            .ok_or(rmcp::ErrorData::invalid_params(
                "Progress token is required",
                None,
            ))?;

        for step in 0..4 {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let _ = client
                .notify_progress(ProgressNotificationParam {
                    progress_token: progress_token.clone(),
                    progress: step as f64,
                    total: Some(4.0),
                    message: Some("working".into()),
                })
                .await;
        }

        Ok(())
    }

    #[tool]
    async fn delayed_with_unrelated_progress(
        &self,
        client: Peer<RoleServer>,
    ) -> Result<(), rmcp::ErrorData> {
        for step in 0..4 {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let _ = client
                .notify_progress(ProgressNotificationParam {
                    progress_token: rmcp::model::ProgressToken(
                        rmcp::model::NumberOrString::Number(999_999),
                    ),
                    progress: step as f64,
                    total: Some(4.0),
                    message: Some("unrelated".into()),
                })
                .await;
        }

        Ok(())
    }
}

async fn start_pair()
-> anyhow::Result<rmcp::service::RunningService<rmcp::RoleClient, ProgressCountingClient>> {
    let server = ProgressTimeoutServer::new();
    let client = ProgressCountingClient::default();
    let (transport_server, transport_client) = tokio::io::duplex(4096);

    tokio::spawn(async move {
        let service = server.serve(transport_server).await?;
        service.waiting().await?;
        anyhow::Ok(())
    });

    Ok(client.serve(transport_client).await?)
}

async fn call_tool_with_options(
    client: &rmcp::service::RunningService<rmcp::RoleClient, ProgressCountingClient>,
    name: &str,
    options: PeerRequestOptions,
) -> Result<rmcp::model::ServerResult, ServiceError> {
    client
        .send_request_with_option(
            ClientRequest::CallToolRequest(Request::new(CallToolRequestParams::new(
                name.to_owned(),
            ))),
            options,
        )
        .await?
        .await_response()
        .await
}

#[tokio::test]
async fn request_timeout_still_expires_without_progress() -> anyhow::Result<()> {
    let client = start_pair().await?;
    let result = call_tool_with_options(
        &client,
        "delayed_without_progress",
        PeerRequestOptions::with_timeout(Duration::from_millis(75)),
    )
    .await;

    assert!(matches!(result, Err(ServiceError::Timeout { .. })));
    Ok(())
}

#[tokio::test]
async fn progress_does_not_reset_timeout_by_default() -> anyhow::Result<()> {
    let client = start_pair().await?;
    let result = call_tool_with_options(
        &client,
        "delayed_with_progress",
        PeerRequestOptions::with_timeout(Duration::from_millis(75)),
    )
    .await;

    assert!(matches!(result, Err(ServiceError::Timeout { .. })));
    Ok(())
}

#[tokio::test]
async fn matching_progress_resets_timeout_when_enabled() -> anyhow::Result<()> {
    let client = start_pair().await?;
    let result = call_tool_with_options(
        &client,
        "delayed_with_progress",
        PeerRequestOptions::with_timeout(Duration::from_millis(75)).reset_timeout_on_progress(),
    )
    .await;

    assert!(result.is_ok());
    assert!(client.service().progress_count.load(Ordering::SeqCst) > 0);
    Ok(())
}

#[tokio::test]
async fn max_total_timeout_wins_over_progress_reset() -> anyhow::Result<()> {
    let client = start_pair().await?;
    let result = call_tool_with_options(
        &client,
        "delayed_with_progress",
        PeerRequestOptions::with_timeout(Duration::from_millis(75))
            .reset_timeout_on_progress()
            .with_max_total_timeout(Duration::from_millis(125)),
    )
    .await;

    assert!(matches!(result, Err(ServiceError::Timeout { .. })));
    Ok(())
}

#[tokio::test]
async fn unrelated_progress_does_not_reset_timeout() -> anyhow::Result<()> {
    let client = start_pair().await?;
    let result = call_tool_with_options(
        &client,
        "delayed_with_unrelated_progress",
        PeerRequestOptions::with_timeout(Duration::from_millis(75)).reset_timeout_on_progress(),
    )
    .await;

    assert!(matches!(result, Err(ServiceError::Timeout { .. })));
    Ok(())
}
