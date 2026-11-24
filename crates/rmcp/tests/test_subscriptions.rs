#![cfg(all(
    not(feature = "local"),
    feature = "client",
    feature = "server",
    feature = "transport-io"
))]

use std::{
    num::NonZeroUsize,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use rmcp::{
    ClientHandler, ClientServiceExt, ServerHandler, ServiceExt,
    model::{
        ClientNotification, ClientRequest, DiscoverResult, GetMeta, Implementation,
        ListToolsResult, NotificationMetaObject, PaginatedRequestParams,
        PromptListChangedNotification, ProtocolVersion, RequestId, ServerCapabilities,
        ServerConfig, ServerNotification, ServerResult, SubscriptionFilter,
        SubscriptionsAcknowledgedNotification, SubscriptionsAcknowledgedNotificationParams,
        SubscriptionsListenResult, ToolListChangedNotification,
    },
    service::{
        NotificationContext, RequestContext, RoleClient, RoleServer, SubscriptionContext,
        SubscriptionEnd, SubscriptionSendError, SubscriptionSink,
    },
};
use serde_json::json;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    sync::{Mutex, Notify},
};

struct ToolsOnlyServer;

#[derive(Clone)]
struct CountingClient {
    tool_changes: Arc<AtomicUsize>,
}

impl ClientHandler for CountingClient {
    async fn on_tool_list_changed(&self, _context: NotificationContext<RoleClient>) {
        self.tool_changes.fetch_add(1, Ordering::Relaxed);
    }
}

impl ServerHandler for ToolsOnlyServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_tool_list_changed()
                .build(),
        )
        .with_server_info(Implementation::new("tools-only-server", "1.0.0"))
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
            .expect("accepted tool notification");
        assert!(matches!(
            context.sink().notify_prompt_list_changed().await,
            Err(SubscriptionSendError::NotificationNotAccepted(
                "notifications/prompts/list_changed"
            ))
        ));
        Ok(())
    }
}

struct ToolsAndPromptsServer;

impl ServerHandler for ToolsAndPromptsServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_tool_list_changed()
                .enable_prompts()
                .enable_prompts_list_changed()
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
        if context.accepted().tools_list_changed == Some(true) {
            context
                .sink()
                .notify_tool_list_changed()
                .await
                .expect("send tool notification");
        }
        if context.accepted().prompts_list_changed == Some(true) {
            context
                .sink()
                .notify_prompt_list_changed()
                .await
                .expect("send prompt notification");
        }
        Ok(())
    }
}

struct ResourceSubscriptionServer;

impl ServerHandler for ResourceSubscriptionServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_resources()
                .enable_resources_subscribe()
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
            .notify_resource_updated("file:///accepted")
            .await
            .expect("accepted URI");
        assert!(matches!(
            context
                .sink()
                .notify_resource_updated("file:///not-requested")
                .await,
            Err(SubscriptionSendError::NotificationNotAccepted(
                "notifications/resources/updated"
            ))
        ));
        Ok(())
    }
}

struct RemoteCancellationServer;

impl ServerHandler for RemoteCancellationServer {
    fn accepted_subscription_filter(
        &self,
        requested: &SubscriptionFilter,
    ) -> Option<SubscriptionFilter> {
        Some(requested.clone())
    }

    async fn listen(&self, context: SubscriptionContext) -> Result<(), rmcp::ErrorData> {
        context
            .request_context()
            .peer
            .notify_cancelled(rmcp::model::CancelledNotificationParam::new(
                Some(context.sink().id().clone()),
                Some("server shutdown".to_owned()),
            ))
            .await
            .expect("send server cancellation");
        std::future::pending().await
    }
}

struct FloodServer;

impl ServerHandler for FloodServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
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
        Some(requested.clone())
    }

    async fn listen(&self, context: SubscriptionContext) -> Result<(), rmcp::ErrorData> {
        for _ in 0..10 {
            if context.sink().notify_tool_list_changed().await.is_err() {
                break;
            }
        }
        context.cancelled().await;
        Ok(())
    }
}

#[derive(Clone)]
struct ClosedSinkServer {
    sink: Arc<Mutex<Option<SubscriptionSink>>>,
}

struct LeakyServer;

impl ServerHandler for LeakyServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
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
        let mut notification =
            ServerNotification::PromptListChangedNotification(PromptListChangedNotification {
                method: Default::default(),
                extensions: Default::default(),
            });
        notification
            .get_meta_mut()
            .set_subscription_id(context.sink().id().clone());
        context
            .request_context()
            .peer
            .send_notification(notification)
            .await
            .expect("send deliberately invalid notification");
        std::future::pending().await
    }
}

struct MalformedAcknowledgmentServer {
    cancelled: Arc<Notify>,
}

impl rmcp::service::Service<RoleServer> for MalformedAcknowledgmentServer {
    async fn handle_request(
        &self,
        request: ClientRequest,
        context: RequestContext<RoleServer>,
    ) -> Result<ServerResult, rmcp::ErrorData> {
        match request {
            ClientRequest::DiscoverRequest(_) => Ok(ServerResult::DiscoverResult(
                DiscoverResult::new(
                    vec![ProtocolVersion::V_2026_07_28],
                    ServerCapabilities::builder()
                        .enable_tools()
                        .enable_tool_list_changed()
                        .enable_prompts()
                        .enable_prompts_list_changed()
                        .build(),
                )
                .with_server_info(Implementation::new("malformed-ack-server", "1.0.0")),
            )),
            ClientRequest::SubscriptionsListenRequest(_) => {
                let mut acknowledgment = SubscriptionsAcknowledgedNotification::new(
                    SubscriptionsAcknowledgedNotificationParams::new(
                        SubscriptionFilter::builder().prompts_list_changed().build(),
                    ),
                );
                let mut meta = NotificationMetaObject::new();
                meta.set_subscription_id(context.id.clone());
                acknowledgment.extensions.insert(meta);
                context
                    .peer
                    .send_notification(ServerNotification::SubscriptionsAcknowledgedNotification(
                        acknowledgment,
                    ))
                    .await
                    .map_err(|error| rmcp::ErrorData::internal_error(error.to_string(), None))?;
                context.ct.cancelled().await;
                Ok(ServerResult::SubscriptionsListenResult(
                    SubscriptionsListenResult::complete(context.id),
                ))
            }
            _ => Err(rmcp::ErrorData::invalid_request(
                "unexpected test request",
                None,
            )),
        }
    }

    async fn handle_notification(
        &self,
        notification: ClientNotification,
        _context: NotificationContext<RoleServer>,
    ) -> Result<(), rmcp::ErrorData> {
        if matches!(notification, ClientNotification::CancelledNotification(_)) {
            self.cancelled.notify_one();
        }
        Ok(())
    }

    fn get_info(&self) -> rmcp::model::ServerConfig {
        ServerConfig::default()
    }
}

impl ServerHandler for ClosedSinkServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
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
        self.sink.lock().await.replace(context.sink().clone());
        Ok(())
    }
}

#[derive(Clone)]
struct CancellationServer {
    cancelled: Arc<Notify>,
}

impl ServerHandler for CancellationServer {
    fn accepted_subscription_filter(
        &self,
        requested: &SubscriptionFilter,
    ) -> Option<SubscriptionFilter> {
        Some(requested.clone())
    }

    async fn listen(&self, context: SubscriptionContext) -> Result<(), rmcp::ErrorData> {
        context.cancelled().await;
        self.cancelled.notify_one();
        Ok(())
    }
}

#[derive(Default)]
struct NotificationFenceServer {
    send_active: Notify,
}

fn correlated_tool_list_changed(id: i64) -> ServerNotification {
    let mut notification =
        ServerNotification::ToolListChangedNotification(ToolListChangedNotification {
            method: Default::default(),
            extensions: Default::default(),
        });
    notification
        .get_meta_mut()
        .set_subscription_id(RequestId::Number(id));
    notification
}

impl ServerHandler for NotificationFenceServer {
    fn get_info(&self) -> ServerConfig {
        ToolsOnlyServer.get_info()
    }

    fn accepted_subscription_filter(
        &self,
        requested: &SubscriptionFilter,
    ) -> Option<SubscriptionFilter> {
        Some(requested.clone())
    }

    async fn listen(&self, context: SubscriptionContext) -> Result<(), rmcp::ErrorData> {
        if *context.sink().id() == RequestId::Number(1) {
            // The serve loop removes the ID before it cancels this token.
            context.cancelled().await;
            let peer = &context.request_context().peer;
            // Send through the peer, bypassing the sink's closed-state check,
            // so the stale frame actually reaches the serve-loop queue.
            peer.send_notification(correlated_tool_list_changed(1))
                .await
                .unwrap();
            peer.notify_tool_list_changed().await.unwrap();
            self.send_active.notify_one();
        } else {
            self.send_active.notified().await;
            context.sink().notify_tool_list_changed().await.unwrap();
        }
        Ok(())
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, rmcp::ErrorData> {
        // The client has already received the normal listen response for ID 2.
        context
            .peer
            .send_notification(correlated_tool_list_changed(2))
            .await
            .unwrap();
        Ok(ListToolsResult::default())
    }
}

#[rstest::rstest]
#[case::cancel_then_complete(true)]
#[case::complete_without_cancellation(false)]
#[tokio::test]
async fn stdio_fences_inactive_subscriptions(#[case] cancel_first: bool) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(5), async {
        let (server_io, client_io) = tokio::io::duplex(4096);
        let server = tokio::spawn(async move {
            let handler = NotificationFenceServer::default();
            if !cancel_first {
                handler.send_active.notify_one();
            }
            handler.serve(server_io).await?.waiting().await?;
            anyhow::Ok(())
        });
        let (read, mut write) = tokio::io::split(client_io);
        let mut read = BufReader::new(read).lines();
        let meta = json!({
            "io.modelcontextprotocol/protocolVersion": "2026-07-28",
            "io.modelcontextprotocol/clientInfo": {"name":"test","version":"1.0"},
            "io.modelcontextprotocol/clientCapabilities": {}
        });
        let send = |message: serde_json::Value| format!("{message}\n").into_bytes();
        let decoded = |line: Option<String>| -> anyhow::Result<serde_json::Value> {
            Ok(serde_json::from_str(&line.expect("server frame"))?)
        };
        const SUBSCRIPTION_ID: &str = "io.modelcontextprotocol/subscriptionId";

        write
            .write_all(&send(json!({
                "jsonrpc":"2.0", "id":0, "method":"server/discover", "params":{"_meta":meta}
            })))
            .await?;
        assert!(decoded(read.next_line().await?)?.get("result").is_some());

        let listen_ids: &[_] = if cancel_first { &[1, 2] } else { &[2] };
        for &id in listen_ids {
            write
                .write_all(&send(json!({
                    "jsonrpc":"2.0", "id":id, "method":"subscriptions/listen",
                    "params":{"_meta":meta, "notifications":{"toolsListChanged":true}}
                })))
                .await?;
            let ack = decoded(read.next_line().await?)?;
            assert_eq!(ack["method"], "notifications/subscriptions/acknowledged");
            assert_eq!(ack["params"]["_meta"][SUBSCRIPTION_ID], id);
        }

        if cancel_first {
            write
                .write_all(&send(json!({
                    "jsonrpc":"2.0", "method":"notifications/cancelled", "params":{"requestId":1}
                })))
                .await?;
            let uncorrelated = decoded(read.next_line().await?)?;
            assert_eq!(uncorrelated["method"], "notifications/tools/list_changed");
            assert!(uncorrelated["params"]["_meta"][SUBSCRIPTION_ID].is_null());
        }
        let active = decoded(read.next_line().await?)?;
        assert_eq!(active["method"], "notifications/tools/list_changed");
        assert_eq!(active["params"]["_meta"][SUBSCRIPTION_ID], 2);
        let completed = decoded(read.next_line().await?)?;
        assert_eq!(completed["id"], 2);
        assert!(completed.get("result").is_some());

        write
            .write_all(&send(json!({
                "jsonrpc":"2.0", "id":3, "method":"tools/list", "params":{"_meta":meta}
            })))
            .await?;
        let tools = decoded(read.next_line().await?)?;
        assert_eq!(tools["id"], 3, "expected tools/list response, got {tools}");
        assert_eq!(tools["result"]["tools"], json!([]));

        write.shutdown().await?;
        server.await??;
        Ok(())
    })
    .await?
}

#[derive(Clone)]
struct AbruptServer {
    started: Arc<Notify>,
}

impl ServerHandler for AbruptServer {
    fn accepted_subscription_filter(
        &self,
        requested: &SubscriptionFilter,
    ) -> Option<SubscriptionFilter> {
        Some(requested.clone())
    }

    async fn listen(&self, _context: SubscriptionContext) -> Result<(), rmcp::ErrorData> {
        self.started.notify_one();
        std::future::pending().await
    }
}

async fn modern_client<S: ServerHandler>(
    server: S,
) -> anyhow::Result<rmcp::service::RunningService<rmcp::RoleClient, ()>> {
    let (server_transport, client_transport) = tokio::io::duplex(16 * 1024);
    tokio::spawn(async move {
        let server = server.serve(server_transport).await?;
        server.waiting().await?;
        anyhow::Ok(())
    });
    ().serve_with_lifecycle(
        client_transport,
        rmcp::ClientLifecycleMode::Discover {
            preferred_versions: vec![ProtocolVersion::V_2026_07_28],
        },
    )
    .await
    .map_err(Into::into)
}

#[tokio::test]
async fn listen_exposes_acknowledged_filter_and_graceful_result() -> anyhow::Result<()> {
    let client = modern_client(ToolsOnlyServer).await?;
    let mut subscription = client
        .listen(
            SubscriptionFilter::builder()
                .tools_list_changed()
                .prompts_list_changed()
                .build(),
        )
        .await?;

    assert_eq!(
        subscription.acknowledged(),
        &SubscriptionFilter::builder().tools_list_changed().build()
    );

    let notification = tokio::time::timeout(Duration::from_secs(5), subscription.next())
        .await??
        .expect("tool notification");
    assert!(matches!(
        notification,
        ServerNotification::ToolListChangedNotification(_)
    ));
    assert_eq!(
        notification.get_meta().subscription_id(),
        Some(subscription.id().clone())
    );

    assert!(
        tokio::time::timeout(Duration::from_secs(5), subscription.next())
            .await??
            .is_none()
    );
    let Some(SubscriptionEnd::Graceful(result)) = subscription.end() else {
        panic!("expected graceful final result");
    };
    assert_eq!(
        result.meta.subscription_id().as_ref(),
        Some(subscription.id())
    );
    assert_eq!(
        result
            .meta
            .server_info()
            .expect("graceful result should contain valid server info"),
        Implementation::new("tools-only-server", "1.0.0")
    );

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn typed_subscription_notifications_do_not_reach_handler_callbacks() -> anyhow::Result<()> {
    let (server_transport, client_transport) = tokio::io::duplex(16 * 1024);
    tokio::spawn(async move {
        let server = ToolsOnlyServer.serve(server_transport).await?;
        server.waiting().await?;
        anyhow::Ok(())
    });
    let tool_changes = Arc::new(AtomicUsize::new(0));
    let client = CountingClient {
        tool_changes: tool_changes.clone(),
    }
    .serve_with_lifecycle(
        client_transport,
        rmcp::ClientLifecycleMode::Discover {
            preferred_versions: vec![ProtocolVersion::V_2026_07_28],
        },
    )
    .await?;
    let mut subscription = client
        .listen(SubscriptionFilter::builder().tools_list_changed().build())
        .await?;

    assert!(subscription.next().await?.is_some());
    assert!(subscription.next().await?.is_none());
    tokio::task::yield_now().await;
    assert_eq!(tool_changes.load(Ordering::Relaxed), 0);

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn discover_lifecycle_allows_subscriptions_with_older_application_version()
-> anyhow::Result<()> {
    let (server_transport, client_transport) = tokio::io::duplex(16 * 1024);
    tokio::spawn(async move {
        let server = ToolsOnlyServer.serve(server_transport).await?;
        server.waiting().await?;
        anyhow::Ok(())
    });
    let client = ()
        .serve_with_lifecycle(
            client_transport,
            rmcp::ClientLifecycleMode::Discover {
                preferred_versions: vec![ProtocolVersion::V_2025_11_25],
            },
        )
        .await?;
    let mut subscription = client
        .listen(SubscriptionFilter::builder().tools_list_changed().build())
        .await?;

    assert!(subscription.next().await?.is_some());
    assert!(subscription.next().await?.is_none());

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn concurrent_subscriptions_are_demultiplexed_by_request_id() -> anyhow::Result<()> {
    let client = modern_client(ToolsAndPromptsServer).await?;
    let (tools, prompts) = tokio::join!(
        client.listen(SubscriptionFilter::builder().tools_list_changed().build()),
        client.listen(SubscriptionFilter::builder().prompts_list_changed().build())
    );
    let mut tools = tools?;
    let mut prompts = prompts?;

    let tool_notification = tools.next().await?.expect("tool notification");
    let prompt_notification = prompts.next().await?.expect("prompt notification");
    assert!(matches!(
        tool_notification,
        ServerNotification::ToolListChangedNotification(_)
    ));
    assert!(matches!(
        prompt_notification,
        ServerNotification::PromptListChangedNotification(_)
    ));
    assert_ne!(tools.id(), prompts.id());

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn stdio_cancellation_sends_cancelled_for_the_listen_request() -> anyhow::Result<()> {
    let cancelled = Arc::new(Notify::new());
    let client = modern_client(CancellationServer {
        cancelled: cancelled.clone(),
    })
    .await?;
    let mut subscription = client.listen(SubscriptionFilter::new()).await?;

    subscription.cancel().await?;
    assert!(matches!(
        subscription.end(),
        Some(SubscriptionEnd::Cancelled)
    ));
    tokio::time::timeout(Duration::from_secs(5), cancelled.notified()).await?;

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn resource_updates_are_filtered_by_exact_uri_membership() -> anyhow::Result<()> {
    let client = modern_client(ResourceSubscriptionServer).await?;
    let mut subscription = client
        .listen(
            SubscriptionFilter::builder()
                .resource_subscription("file:///accepted")
                .build(),
        )
        .await?;

    let notification = subscription.next().await?.expect("resource update");
    let ServerNotification::ResourceUpdatedNotification(update) = notification else {
        panic!("expected resource update");
    };
    assert_eq!(update.params.uri, "file:///accepted");
    assert!(subscription.next().await?.is_none());

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn stdio_server_cancellation_ends_the_matching_subscription() -> anyhow::Result<()> {
    let client = modern_client(RemoteCancellationServer).await?;
    let mut subscription = client.listen(SubscriptionFilter::new()).await?;

    assert!(subscription.next().await?.is_none());
    assert!(matches!(
        subscription.end(),
        Some(SubscriptionEnd::Cancelled)
    ));

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn slow_consumer_reports_subscription_lag() -> anyhow::Result<()> {
    let client = modern_client(FloodServer).await?;
    let mut subscription = client
        .listen_with_capacity(
            SubscriptionFilter::builder().tools_list_changed().build(),
            NonZeroUsize::MIN,
        )
        .await?;

    tokio::time::sleep(Duration::from_millis(50)).await;
    while subscription.next().await?.is_some() {}
    assert!(
        matches!(
            subscription.end(),
            Some(SubscriptionEnd::Lagged { capacity: 1 })
        ),
        "unexpected end: {:?}",
        subscription.end()
    );

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn sink_rejects_notifications_after_graceful_completion() -> anyhow::Result<()> {
    let sink = Arc::new(Mutex::new(None));
    let client = modern_client(ClosedSinkServer { sink: sink.clone() }).await?;
    let mut subscription = client
        .listen(SubscriptionFilter::builder().tools_list_changed().build())
        .await?;
    assert!(subscription.next().await?.is_none());

    let sink = sink.lock().await.clone().expect("captured sink");
    assert!(matches!(
        sink.notify_tool_list_changed().await,
        Err(SubscriptionSendError::SubscriptionClosed)
    ));

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn client_rejects_notifications_outside_the_acknowledged_filter() -> anyhow::Result<()> {
    let client = modern_client(LeakyServer).await?;
    let mut subscription = client
        .listen(SubscriptionFilter::builder().tools_list_changed().build())
        .await?;

    assert!(matches!(
        subscription.next().await,
        Err(rmcp::ServiceError::UnexpectedResponse)
    ));
    assert!(matches!(subscription.end(), Some(SubscriptionEnd::Abrupt)));

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn malformed_acknowledgment_cancels_pending_listen_request() -> anyhow::Result<()> {
    let (server_transport, client_transport) = tokio::io::duplex(16 * 1024);
    let cancelled = Arc::new(Notify::new());
    let server_cancelled = cancelled.clone();
    tokio::spawn(async move {
        let server = MalformedAcknowledgmentServer {
            cancelled: server_cancelled,
        }
        .serve(server_transport)
        .await?;
        server.waiting().await?;
        anyhow::Ok(())
    });
    let client = ()
        .serve_with_lifecycle(
            client_transport,
            rmcp::ClientLifecycleMode::Discover {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            },
        )
        .await?;

    assert!(matches!(
        client
            .listen(SubscriptionFilter::builder().tools_list_changed().build())
            .await,
        Err(rmcp::ServiceError::UnexpectedResponse)
    ));
    tokio::time::timeout(Duration::from_secs(5), cancelled.notified()).await?;

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn transport_close_without_final_result_is_abrupt() -> anyhow::Result<()> {
    let (server_transport, client_transport) = tokio::io::duplex(16 * 1024);
    let started = Arc::new(Notify::new());
    let server_started = started.clone();
    tokio::spawn(async move {
        let server = AbruptServer {
            started: server_started.clone(),
        }
        .serve(server_transport)
        .await?;
        server_started.notified().await;
        server.cancel().await?;
        anyhow::Ok(())
    });
    let client = ()
        .serve_with_lifecycle(
            client_transport,
            rmcp::ClientLifecycleMode::Discover {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            },
        )
        .await?;
    let mut subscription = client.listen(SubscriptionFilter::new()).await?;

    assert!(
        tokio::time::timeout(Duration::from_secs(5), subscription.next())
            .await??
            .is_none()
    );
    assert!(matches!(subscription.end(), Some(SubscriptionEnd::Abrupt)));
    Ok(())
}
