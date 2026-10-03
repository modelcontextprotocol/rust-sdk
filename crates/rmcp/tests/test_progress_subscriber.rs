#![cfg(not(feature = "local"))]
use std::time::Duration;

use futures::StreamExt;
use rmcp::{
    ClientHandler, Peer, RoleServer, ServerHandler, ServiceExt,
    handler::{client::progress::ProgressDispatcher, server::tool::ToolRouter},
    model::{
        CallToolRequestParams, ClientRequest, NumberOrString, ProgressNotificationParam,
        ProgressToken, Request, RequestMetaObject,
    },
    service::PeerRequestOptions,
    tool, tool_handler, tool_router,
};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

pub struct MyClient {
    progress_handler: ProgressDispatcher,
}

impl MyClient {
    pub fn new() -> Self {
        Self {
            progress_handler: ProgressDispatcher::new(),
        }
    }
}

impl Default for MyClient {
    fn default() -> Self {
        Self::new()
    }
}

impl ClientHandler for MyClient {
    async fn on_progress(
        &self,
        params: rmcp::model::ProgressNotificationParam,
        _context: rmcp::service::NotificationContext<rmcp::RoleClient>,
    ) {
        tracing::info!("Received progress notification: {:?}", params);
        self.progress_handler.handle_notification(params).await;
    }
}

pub struct MyServer {
    #[expect(dead_code, reason = "tool_handler macro accesses this router field")]
    tool_router: ToolRouter<Self>,
}

impl MyServer {
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }
}

impl Default for MyServer {
    fn default() -> Self {
        Self::new()
    }
}

#[tool_router]
impl MyServer {
    #[tool]
    pub async fn some_progress(
        meta: RequestMetaObject,
        client: Peer<RoleServer>,
    ) -> Result<(), rmcp::ErrorData> {
        let progress_token = meta
            .get_progress_token()
            .ok_or(rmcp::ErrorData::invalid_params(
                "Progress token is required for this tool",
                None,
            ))?;
        for step in 0..10 {
            let _ = client
                .notify_progress(
                    ProgressNotificationParam::new(progress_token.clone(), step as f64)
                        .with_total(10.0)
                        .with_message("Some message"),
                )
                .await;
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }
        Ok(())
    }
}

#[tool_handler]
impl ServerHandler for MyServer {}

#[tokio::test]
async fn test_progress_subscriber() -> anyhow::Result<()> {
    let _ = tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "debug".to_string().into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .try_init();
    let client = MyClient::new();

    let server = MyServer::new();
    let (transport_server, transport_client) = tokio::io::duplex(4096);
    tokio::spawn(async move {
        let service = server.serve(transport_server).await?;
        service.waiting().await?;
        anyhow::Ok(())
    });
    let client_service = client.serve(transport_client).await?;
    let handle = client_service
        .send_cancellable_request(
            ClientRequest::CallToolRequest(Request::new(CallToolRequestParams::new(
                "some_progress",
            ))),
            PeerRequestOptions::no_options(),
        )
        .await?;
    let mut progress_subscriber = client_service
        .service()
        .progress_handler
        .subscribe(handle.progress_token.clone())
        .await;
    tokio::spawn(async move {
        while let Some(notification) = progress_subscriber.next().await {
            tracing::info!("Progress notification: {:?}", notification);
        }
    });
    let _response = handle.await_response().await?;

    // Simulate some delay to allow the async task to complete
    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
    Ok(())
}

fn progress(token: &ProgressToken, step: u32) -> ProgressNotificationParam {
    ProgressNotificationParam::new(token.clone(), step as f64)
}

// A subscriber that is not keeping up must only back-pressure its own token,
// not every other subscription sharing the dispatcher.
#[tokio::test]
async fn test_slow_subscriber_does_not_block_other_tokens() -> anyhow::Result<()> {
    const SLOW_NOTIFICATIONS: u32 = 64;
    let dispatcher = ProgressDispatcher::new();
    let slow_token = ProgressToken(NumberOrString::Number(1));
    let mut slow = dispatcher.subscribe(slow_token.clone()).await;

    // Deliver more notifications than the slow subscriber can buffer while it is not polled.
    let producer = tokio::spawn({
        let dispatcher = dispatcher.clone();
        let slow_token = slow_token.clone();
        async move {
            for step in 0..SLOW_NOTIFICATIONS {
                dispatcher
                    .handle_notification(progress(&slow_token, step))
                    .await;
            }
        }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !producer.is_finished(),
        "slow subscriber should be applying back-pressure"
    );

    let fast_token = ProgressToken(NumberOrString::Number(2));
    let mut fast = tokio::time::timeout(
        Duration::from_secs(1),
        dispatcher.subscribe(fast_token.clone()),
    )
    .await
    .expect("subscribe() stalled behind an unrelated slow subscriber");
    tokio::time::timeout(
        Duration::from_secs(1),
        dispatcher.handle_notification(progress(&fast_token, 7)),
    )
    .await
    .expect("delivery to an unrelated token stalled behind a slow subscriber");
    let received = tokio::time::timeout(Duration::from_secs(1), fast.next())
        .await
        .expect("fast subscriber did not receive its notification")
        .expect("fast subscriber stream ended");
    assert_eq!(received.progress, 7.0);

    // Once the slow subscriber catches up it still gets every notification, in order.
    for step in 0..SLOW_NOTIFICATIONS {
        let received = tokio::time::timeout(Duration::from_secs(1), slow.next())
            .await
            .expect("slow subscriber did not receive a queued notification")
            .expect("slow subscriber stream ended");
        assert_eq!(received.progress, step as f64);
    }
    tokio::time::timeout(Duration::from_secs(1), producer).await??;
    Ok(())
}

#[tokio::test]
async fn test_resubscribing_a_dropped_token_keeps_the_new_subscriber() {
    let dispatcher = ProgressDispatcher::new();
    let token = ProgressToken(NumberOrString::Number(1));
    drop(dispatcher.subscribe(token.clone()).await);
    let mut subscriber = dispatcher.subscribe(token.clone()).await;
    // Give any deferred cleanup of the dropped subscriber a chance to run.
    tokio::time::sleep(Duration::from_millis(50)).await;

    dispatcher.handle_notification(progress(&token, 3)).await;
    let received = tokio::time::timeout(Duration::from_secs(1), subscriber.next())
        .await
        .expect("new subscriber did not receive its notification")
        .expect("dropping the old subscriber unregistered the new one");
    assert_eq!(received.progress, 3.0);
}

#[tokio::test]
async fn test_dropping_a_replaced_subscriber_keeps_its_replacement() {
    let dispatcher = ProgressDispatcher::new();
    let token = ProgressToken(NumberOrString::Number(1));
    let replaced = dispatcher.subscribe(token.clone()).await;
    let mut replacement = dispatcher.subscribe(token.clone()).await;
    drop(replaced);
    tokio::time::sleep(Duration::from_millis(50)).await;

    dispatcher.handle_notification(progress(&token, 5)).await;
    let received = tokio::time::timeout(Duration::from_secs(1), replacement.next())
        .await
        .expect("replacement subscriber did not receive its notification")
        .expect("dropping the replaced subscriber unregistered its replacement");
    assert_eq!(received.progress, 5.0);
}
