use std::{borrow::Cow, time::Duration};

use rmcp::{
    ErrorData, ServerHandler,
    model::{ProtocolVersion, ServerCapabilities, ServerInfo, SubscriptionFilter},
    service::SubscriptionContext,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
struct SubscriptionServer;

impl ServerHandler for SubscriptionServer {
    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Borrowed(&[ProtocolVersion::V_2026_07_28])
    }

    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_tool_list_changed()
                .build(),
        )
    }

    fn accepted_subscription_filter(
        &self,
        requested: &SubscriptionFilter,
    ) -> Option<SubscriptionFilter> {
        Some(requested.supported_by(&self.get_info().capabilities))
    }

    async fn listen(&self, context: SubscriptionContext) -> Result<(), ErrorData> {
        loop {
            tokio::select! {
                () = context.cancelled() => return Ok(()),
                () = tokio::time::sleep(Duration::from_secs(2)) => {
                    context
                        .sink()
                        .notify_tool_list_changed()
                        .await
                        .map_err(|error| {
                            ErrorData::internal_error(error.to_string(), None)
                        })?;
                }
            }
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cancellation_token = CancellationToken::new();
    let service: StreamableHttpService<SubscriptionServer, LocalSessionManager> =
        StreamableHttpService::new(
            || Ok(SubscriptionServer),
            Default::default(),
            StreamableHttpServerConfig::default()
                .with_legacy_session_mode(false)
                .with_sse_keep_alive(Some(Duration::from_secs(10)))
                .with_cancellation_token(cancellation_token.child_token()),
        );
    let router = axum::Router::new().nest_service("/mcp", service);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:8000").await?;

    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            let _ = tokio::signal::ctrl_c().await;
            cancellation_token.cancel();
        })
        .await?;
    Ok(())
}
