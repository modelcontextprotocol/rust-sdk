#![cfg(all(
    not(feature = "local"),
    feature = "client",
    feature = "server",
    feature = "transport-streamable-http-client-reqwest",
    feature = "transport-streamable-http-server"
))]

use std::{
    borrow::Cow,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use rmcp::{
    ClientLifecycleMode, ClientServiceExt, ServerHandler,
    model::{
        ClientInfo, ClientRequest, ListToolsRequest, ProtocolVersion, RequestMetaObject,
        ServerCapabilities, ServerInfo, ServerNotification, SubscriptionFilter,
    },
    service::{PeerRequestOptions, SubscriptionContext, SubscriptionEnd},
    transport::{
        StreamableHttpClientTransport,
        streamable_http_client::StreamableHttpClientTransportConfig,
        streamable_http_server::{
            StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
        },
    },
};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
struct HttpSubscriptionServer {
    cancelled: Arc<Notify>,
    started: Arc<Notify>,
    ending: ServerEnding,
}

#[derive(Clone, Copy)]
enum ServerEnding {
    ClientCancellation,
    Graceful,
    Abrupt,
}

impl ServerHandler for HttpSubscriptionServer {
    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Borrowed(&[ProtocolVersion::V_2026_07_28, ProtocolVersion::V_2025_11_25])
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

    async fn listen(&self, context: SubscriptionContext) -> Result<(), rmcp::ErrorData> {
        context
            .sink()
            .notify_tool_list_changed()
            .await
            .expect("send tool notification");
        self.started.notify_one();
        match self.ending {
            ServerEnding::Graceful => Ok(()),
            ServerEnding::ClientCancellation => {
                context.cancelled().await;
                self.cancelled.notify_one();
                Ok(())
            }
            ServerEnding::Abrupt => std::future::pending().await,
        }
    }
}

async fn spawn_server(
    ending: ServerEnding,
) -> (
    String,
    CancellationToken,
    Arc<Notify>,
    Arc<Notify>,
    Arc<AtomicUsize>,
) {
    let cancellation_token = CancellationToken::new();
    let subscription_cancelled = Arc::new(Notify::new());
    let subscription_started = Arc::new(Notify::new());
    let server = HttpSubscriptionServer {
        cancelled: subscription_cancelled.clone(),
        started: subscription_started.clone(),
        ending,
    };
    let service: StreamableHttpService<HttpSubscriptionServer, LocalSessionManager> =
        StreamableHttpService::new(
            move || Ok(server.clone()),
            Default::default(),
            StreamableHttpServerConfig::default()
                .with_legacy_session_mode(true)
                .with_json_response(true)
                .with_sse_keep_alive(Some(Duration::from_millis(50)))
                .with_cancellation_token(cancellation_token.child_token()),
        );
    let get_requests = Arc::new(AtomicUsize::new(0));
    let observed_get_requests = get_requests.clone();
    let router =
        axum::Router::new()
            .nest_service("/mcp", service)
            .layer(axum::middleware::from_fn(
                move |request: axum::extract::Request, next: axum::middleware::Next| {
                    let observed_get_requests = observed_get_requests.clone();
                    async move {
                        if request.method() == axum::http::Method::GET {
                            observed_get_requests.fetch_add(1, Ordering::Relaxed);
                        }
                        next.run(request).await
                    }
                },
            ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener");
    let address = listener.local_addr().expect("listener address");
    tokio::spawn({
        let cancellation_token = cancellation_token.clone();
        async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async move { cancellation_token.cancelled_owned().await })
                .await;
        }
    });
    (
        format!("http://{address}/mcp"),
        cancellation_token,
        subscription_cancelled,
        subscription_started,
        get_requests,
    )
}

#[tokio::test]
async fn modern_http_listen_uses_post_stream_and_cancels_by_closing_it() -> anyhow::Result<()> {
    let (url, server_ct, subscription_cancelled, _, get_requests) =
        spawn_server(ServerEnding::ClientCancellation).await;
    let transport = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(url.clone()),
    );
    let client = ClientInfo::default()
        .serve_with_lifecycle(
            transport,
            ClientLifecycleMode::Discover {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            },
        )
        .await?;
    assert_eq!(get_requests.load(Ordering::Relaxed), 0);
    let mut subscription = client
        .listen(SubscriptionFilter::builder().tools_list_changed().build())
        .await?;

    assert!(matches!(
        subscription.next().await?.expect("tool notification"),
        ServerNotification::ToolListChangedNotification(_)
    ));
    let mut older_version = RequestMetaObject::new();
    older_version.set_protocol_version(ProtocolVersion::V_2025_11_25);
    client
        .send_request_with_option(
            ClientRequest::ListToolsRequest(ListToolsRequest {
                method: Default::default(),
                params: None,
                extensions: Default::default(),
            }),
            PeerRequestOptions::no_options().with_meta(older_version),
        )
        .await?
        .await_response()
        .await?;
    subscription.cancel().await?;
    tokio::time::timeout(Duration::from_secs(5), subscription_cancelled.notified()).await?;

    client.cancel().await?;
    server_ct.cancel();
    Ok(())
}

#[tokio::test]
async fn modern_http_graceful_close_returns_final_listen_result() -> anyhow::Result<()> {
    let (url, server_ct, _, _, _) = spawn_server(ServerEnding::Graceful).await;
    let transport = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(url),
    );
    let client = ClientInfo::default()
        .serve_with_lifecycle(
            transport,
            ClientLifecycleMode::Discover {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            },
        )
        .await?;
    let mut subscription = client
        .listen(SubscriptionFilter::builder().tools_list_changed().build())
        .await?;

    assert!(subscription.next().await?.is_some());
    assert!(subscription.next().await?.is_none());
    assert!(matches!(
        subscription.end(),
        Some(SubscriptionEnd::Graceful(_))
    ));

    client.cancel().await?;
    server_ct.cancel();
    Ok(())
}

#[tokio::test]
async fn modern_http_stream_close_without_result_is_abrupt() -> anyhow::Result<()> {
    let (url, server_ct, _, subscription_started, _) = spawn_server(ServerEnding::Abrupt).await;
    let transport = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(url),
    );
    let client = ClientInfo::default()
        .serve_with_lifecycle(
            transport,
            ClientLifecycleMode::Discover {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            },
        )
        .await?;
    let mut subscription = client
        .listen(SubscriptionFilter::builder().tools_list_changed().build())
        .await?;
    assert!(subscription.next().await?.is_some());
    subscription_started.notified().await;

    server_ct.cancel();
    assert!(
        tokio::time::timeout(Duration::from_secs(5), subscription.next())
            .await??
            .is_none()
    );
    assert!(matches!(subscription.end(), Some(SubscriptionEnd::Abrupt)));

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn modern_http_lifecycle_stays_sessionless_for_older_application_version()
-> anyhow::Result<()> {
    let (url, server_ct, _, _, get_requests) = spawn_server(ServerEnding::ClientCancellation).await;
    let transport = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(url),
    );
    let client = ClientInfo::default()
        .serve_with_lifecycle(
            transport,
            ClientLifecycleMode::Discover {
                preferred_versions: vec![ProtocolVersion::V_2025_11_25],
            },
        )
        .await?;

    client.list_tools(None).await?;
    assert_eq!(get_requests.load(Ordering::Relaxed), 0);

    client.cancel().await?;
    server_ct.cancel();
    Ok(())
}

#[tokio::test]
async fn modern_http_get_and_delete_are_method_not_allowed_in_legacy_session_mode() {
    let (url, server_ct, _, _, _) = spawn_server(ServerEnding::ClientCancellation).await;
    let client = reqwest::Client::new();

    for method in [reqwest::Method::GET, reqwest::Method::DELETE] {
        let response = client
            .request(method, &url)
            .header("Accept", "text/event-stream")
            .header("MCP-Protocol-Version", "2026-07-28")
            .send()
            .await
            .expect("request");
        assert_eq!(response.status(), reqwest::StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(
            response
                .headers()
                .get(reqwest::header::ALLOW)
                .and_then(|value| value.to_str().ok()),
            Some("POST")
        );
    }

    server_ct.cancel();
}
