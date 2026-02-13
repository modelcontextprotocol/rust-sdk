use std::sync::Arc;

use rmcp::{
    ClientHandler, ServerHandler, ServiceExt,
    model::{
        ClientNotification, CustomNotification, ResourceUpdatedNotificationParam,
        ServerCapabilities, ServerInfo, ServerNotification, SubscribeRequestParams,
    },
};
use serde_json::json;
use tokio::sync::{Mutex, Notify};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

pub struct Server {}

impl ServerHandler for Server {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            capabilities: ServerCapabilities::builder()
                .enable_resources()
                .enable_resources_subscribe()
                .enable_resources_list_changed()
                .build(),
            ..Default::default()
        }
    }

    async fn subscribe(
        &self,
        request: rmcp::model::SubscribeRequestParams,
        context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<(), rmcp::ErrorData> {
        let uri = request.uri;
        let peer = context.peer;

        tokio::spawn(async move {
            let span = tracing::info_span!("subscribe", uri = %uri);
            let _enter = span.enter();

            if let Err(e) = peer
                .notify_resource_updated(ResourceUpdatedNotificationParam { uri: uri.clone() })
                .await
            {
                panic!("Failed to send notification: {}", e);
            }
        });

        Ok(())
    }
}

pub struct Client {
    receive_signal: Arc<Notify>,
}

impl ClientHandler for Client {
    async fn on_resource_updated(
        &self,
        params: rmcp::model::ResourceUpdatedNotificationParam,
        _context: rmcp::service::NotificationContext<rmcp::RoleClient>,
    ) {
        let uri = params.uri;
        tracing::info!("Resource updated: {}", uri);
        self.receive_signal.notify_one();
    }
}

#[tokio::test]
async fn test_server_notification() -> anyhow::Result<()> {
    let _ = tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "debug".to_string().into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .try_init();
    let (server_transport, client_transport) = tokio::io::duplex(4096);
    tokio::spawn(async move {
        let server = Server {}.serve(server_transport).await?;
        server.waiting().await?;
        anyhow::Ok(())
    });
    let receive_signal = Arc::new(Notify::new());
    let client = Client {
        receive_signal: receive_signal.clone(),
    }
    .serve(client_transport)
    .await?;
    client
        .subscribe(SubscribeRequestParams {
            meta: None,
            uri: "test://test-resource".to_owned(),
        })
        .await?;
    receive_signal.notified().await;
    client.cancel().await?;
    Ok(())
}

type CustomNotificationPayload = (String, Option<serde_json::Value>);

struct CustomServer {
    receive_signal: Arc<Notify>,
    payload: Arc<Mutex<Option<CustomNotificationPayload>>>,
}

impl ServerHandler for CustomServer {
    async fn on_custom_notification(
        &self,
        notification: CustomNotification,
        _context: rmcp::service::NotificationContext<rmcp::RoleServer>,
    ) {
        let CustomNotification { method, params, .. } = notification;
        *self.payload.lock().await = Some((method, params));
        self.receive_signal.notify_one();
    }
}

#[tokio::test]
async fn test_custom_client_notification_reaches_server() -> anyhow::Result<()> {
    let _ = tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "debug".to_string().into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .try_init();

    let (server_transport, client_transport) = tokio::io::duplex(4096);
    let receive_signal = Arc::new(Notify::new());
    let payload = Arc::new(Mutex::new(None));

    {
        let receive_signal = receive_signal.clone();
        let payload = payload.clone();
        tokio::spawn(async move {
            let server = CustomServer {
                receive_signal,
                payload,
            }
            .serve(server_transport)
            .await?;
            server.waiting().await?;
            anyhow::Ok(())
        });
    }

    let client = ().serve(client_transport).await?;

    client
        .send_notification(ClientNotification::CustomNotification(
            CustomNotification::new("notifications/custom-test", Some(json!({ "foo": "bar" }))),
        ))
        .await?;

    tokio::time::timeout(std::time::Duration::from_secs(5), receive_signal.notified()).await?;

    let (method, params) = payload.lock().await.take().expect("payload set");
    assert_eq!("notifications/custom-test", method);
    assert_eq!(Some(json!({ "foo": "bar" })), params);

    client.cancel().await?;
    Ok(())
}

struct CustomServerNotifier;

impl ServerHandler for CustomServerNotifier {
    async fn on_initialized(&self, context: rmcp::service::NotificationContext<rmcp::RoleServer>) {
        let peer = context.peer.clone();
        tokio::spawn(async move {
            peer.send_notification(ServerNotification::CustomNotification(
                CustomNotification::new(
                    "notifications/custom-test",
                    Some(json!({ "hello": "world" })),
                ),
            ))
            .await
            .expect("send custom notification");
        });
    }
}

struct CustomClient {
    receive_signal: Arc<Notify>,
    payload: Arc<Mutex<Option<CustomNotificationPayload>>>,
}

impl ClientHandler for CustomClient {
    async fn on_custom_notification(
        &self,
        notification: CustomNotification,
        _context: rmcp::service::NotificationContext<rmcp::RoleClient>,
    ) {
        let CustomNotification { method, params, .. } = notification;
        *self.payload.lock().await = Some((method, params));
        self.receive_signal.notify_one();
    }
}

#[tokio::test]
async fn test_custom_server_notification_reaches_client() -> anyhow::Result<()> {
    let _ = tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "debug".to_string().into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .try_init();

    let (server_transport, client_transport) = tokio::io::duplex(4096);
    tokio::spawn(async move {
        let server = CustomServerNotifier {}.serve(server_transport).await?;
        server.waiting().await?;
        anyhow::Ok(())
    });

    let receive_signal = Arc::new(Notify::new());
    let payload = Arc::new(Mutex::new(None));

    let client = CustomClient {
        receive_signal: receive_signal.clone(),
        payload: payload.clone(),
    }
    .serve(client_transport)
    .await?;

    tokio::time::timeout(std::time::Duration::from_secs(5), receive_signal.notified()).await?;

    let (method, params) = payload.lock().await.take().expect("payload set");
    assert_eq!("notifications/custom-test", method);
    assert_eq!(Some(json!({ "hello": "world" })), params);

    client.cancel().await?;
    Ok(())
}
