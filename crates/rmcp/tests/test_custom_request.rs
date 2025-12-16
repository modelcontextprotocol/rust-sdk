use std::sync::Arc;

use rmcp::{
    ClientHandler, ServerHandler, ServiceExt,
    model::{
        ClientRequest, ClientResult, CustomRequest, CustomResult, ServerRequest, ServerResult,
    },
};
use serde_json::json;
use tokio::sync::{Mutex, Notify};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

type CustomRequestPayload = (String, Option<serde_json::Value>);

struct CustomRequestServer {
    receive_signal: Arc<Notify>,
    payload: Arc<Mutex<Option<CustomRequestPayload>>>,
}

impl ServerHandler for CustomRequestServer {
    async fn on_custom_request(
        &self,
        request: CustomRequest,
        _context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<CustomResult, rmcp::ErrorData> {
        let CustomRequest { method, params, .. } = request;
        *self.payload.lock().await = Some((method, params));
        self.receive_signal.notify_one();
        Ok(CustomResult::new(json!({ "status": "ok" })))
    }
}

#[tokio::test]
async fn test_custom_client_request_reaches_server() -> anyhow::Result<()> {
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
            let server = CustomRequestServer {
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

    let response = client
        .send_request(ClientRequest::CustomRequest(CustomRequest::new(
            "requests/custom-test",
            Some(json!({ "foo": "bar" })),
        )))
        .await?;

    tokio::time::timeout(std::time::Duration::from_secs(5), receive_signal.notified()).await?;

    let (method, params) = payload.lock().await.take().expect("payload set");
    assert_eq!("requests/custom-test", method);
    assert_eq!(Some(json!({ "foo": "bar" })), params);

    match response {
        ServerResult::CustomResult(result) => {
            assert_eq!(result.0, json!({ "status": "ok" }));
        }
        other => panic!("Expected custom result, got: {other:?}"),
    }

    client.cancel().await?;
    Ok(())
}

struct CustomRequestClient {
    receive_signal: Arc<Notify>,
    payload: Arc<Mutex<Option<CustomRequestPayload>>>,
}

impl ClientHandler for CustomRequestClient {
    async fn on_custom_request(
        &self,
        request: CustomRequest,
        _context: rmcp::service::RequestContext<rmcp::RoleClient>,
    ) -> Result<CustomResult, rmcp::ErrorData> {
        let CustomRequest { method, params, .. } = request;
        *self.payload.lock().await = Some((method, params));
        self.receive_signal.notify_one();
        Ok(CustomResult::new(json!({ "status": "ok" })))
    }
}

struct CustomRequestServerNotifier {
    receive_signal: Arc<Notify>,
    response: Arc<Mutex<Option<Result<serde_json::Value, String>>>>,
}

impl ServerHandler for CustomRequestServerNotifier {
    async fn on_initialized(&self, context: rmcp::service::NotificationContext<rmcp::RoleServer>) {
        let peer = context.peer.clone();
        let receive_signal = self.receive_signal.clone();
        let response = self.response.clone();
        tokio::spawn(async move {
            let result = peer
                .send_request(ServerRequest::CustomRequest(CustomRequest::new(
                    "requests/custom-server",
                    Some(json!({ "ping": "pong" })),
                )))
                .await;
            let payload = match result {
                Ok(ClientResult::CustomResult(result)) => Ok(result.0),
                Ok(other) => Err(format!("Unexpected response: {other:?}")),
                Err(err) => Err(format!("Failed to send request: {err:?}")),
            };
            *response.lock().await = Some(payload);
            receive_signal.notify_one();
        });
    }
}

#[tokio::test]
async fn test_custom_server_request_reaches_client() -> anyhow::Result<()> {
    let _ = tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "debug".to_string().into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .try_init();

    let (server_transport, client_transport) = tokio::io::duplex(4096);
    let response_signal = Arc::new(Notify::new());
    let response = Arc::new(Mutex::new(None));
    tokio::spawn({
        let response_signal = response_signal.clone();
        let response = response.clone();
        async move {
            let server = CustomRequestServerNotifier {
                receive_signal: response_signal,
                response,
            }
            .serve(server_transport)
            .await?;
            server.waiting().await?;
            anyhow::Ok(())
        }
    });

    let receive_signal = Arc::new(Notify::new());
    let payload = Arc::new(Mutex::new(None));

    let client = CustomRequestClient {
        receive_signal: receive_signal.clone(),
        payload: payload.clone(),
    }
    .serve(client_transport)
    .await?;

    tokio::time::timeout(std::time::Duration::from_secs(5), receive_signal.notified()).await?;
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        response_signal.notified(),
    )
    .await?;

    let (method, params) = payload.lock().await.take().expect("payload set");
    assert_eq!("requests/custom-server", method);
    assert_eq!(Some(json!({ "ping": "pong" })), params);

    let response = response.lock().await.take().expect("response set");
    let response = response.expect("custom request response ok");
    assert_eq!(response, json!({ "status": "ok" }));

    client.cancel().await?;
    Ok(())
}
