use std::{collections::HashMap, io, net::SocketAddr, sync::Arc, time::Duration};

use axum::{
    Extension, Json, Router,
    extract::{NestedPath, Query, State},
    http::{StatusCode, request::Parts},
    response::{
        Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{get, post},
};
use futures::{Sink, SinkExt, Stream};
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::{CancellationToken, PollSender};
use tracing::Instrument;

use super::common::{DEFAULT_AUTO_PING_INTERVAL, SessionId, SseServerConfig, session_id};
use crate::{
    RoleServer, Service,
    model::ClientJsonRpcMessage,
    service::{RxJsonRpcMessage, TxJsonRpcMessage, serve_directly_with_ct},
};

type TxStore =
    Arc<tokio::sync::RwLock<HashMap<SessionId, tokio::sync::mpsc::Sender<ClientJsonRpcMessage>>>>;

#[derive(Clone)]
struct App {
    txs: TxStore,
    transport_tx: tokio::sync::mpsc::UnboundedSender<SseServerTransport>,
    post_path: Arc<str>,
    sse_ping_interval: Duration,
}

impl App {
    pub fn new(
        post_path: String,
        sse_ping_interval: Duration,
    ) -> (
        Self,
        tokio::sync::mpsc::UnboundedReceiver<SseServerTransport>,
    ) {
        let (transport_tx, transport_rx) = tokio::sync::mpsc::unbounded_channel();
        (
            Self {
                txs: Default::default(),
                transport_tx,
                post_path: post_path.into(),
                sse_ping_interval,
            },
            transport_rx,
        )
    }
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PostEventQuery {
    pub session_id: String,
}

async fn post_event_handler(
    State(app): State<App>,
    Query(PostEventQuery { session_id }): Query<PostEventQuery>,
    parts: Parts,
    Json(mut message): Json<ClientJsonRpcMessage>,
) -> Result<StatusCode, StatusCode> {
    tracing::debug!(session_id, ?parts, ?message, "new client message");
    let tx = {
        let rg = app.txs.read().await;
        rg.get(session_id.as_str())
            .ok_or(StatusCode::NOT_FOUND)?
            .clone()
    };
    message.insert_extension(parts);
    if tx.send(message).await.is_err() {
        tracing::error!("send message error");
        return Err(StatusCode::GONE);
    }
    Ok(StatusCode::ACCEPTED)
}

async fn sse_handler(
    State(app): State<App>,
    nested_path: Option<Extension<NestedPath>>,
    parts: Parts,
) -> Result<Sse<impl Stream<Item = Result<Event, io::Error>>>, Response<String>> {
    let session = session_id();
    tracing::info!(%session, ?parts, "sse connection");
    use tokio_stream::{StreamExt, wrappers::ReceiverStream};
    use tokio_util::sync::PollSender;
    let (from_client_tx, from_client_rx) = tokio::sync::mpsc::channel(64);
    let (to_client_tx, to_client_rx) = tokio::sync::mpsc::channel(64);
    let to_client_tx_clone = to_client_tx.clone();

    app.txs
        .write()
        .await
        .insert(session.clone(), from_client_tx);
    let session = session.clone();
    let stream = ReceiverStream::new(from_client_rx);
    let sink = PollSender::new(to_client_tx);
    let transport = SseServerTransport {
        stream,
        sink,
        session_id: session.clone(),
        tx_store: app.txs.clone(),
    };
    let transport_send_result = app.transport_tx.send(transport);
    if transport_send_result.is_err() {
        tracing::warn!("send transport out error");
        let mut response =
            Response::new("fail to send out transport, it seems server is closed".to_string());
        *response.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
        return Err(response);
    }
    let nested_path = nested_path.as_deref().map(NestedPath::as_str).unwrap_or("");
    let post_path = app.post_path.as_ref();
    let ping_interval = app.sse_ping_interval;
    let stream = futures::stream::once(futures::future::ok(
        Event::default()
            .event("endpoint")
            .data(format!("{nested_path}{post_path}?sessionId={session}")),
    ))
    .chain(ReceiverStream::new(to_client_rx).map(|message| {
        match serde_json::to_string(&message) {
            Ok(bytes) => Ok(Event::default().event("message").data(&bytes)),
            Err(e) => Err(io::Error::new(io::ErrorKind::InvalidData, e)),
        }
    }));

    tokio::spawn(async move {
        // Wait for connection closure
        to_client_tx_clone.closed().await;

        // Clean up session
        let session_id = session.clone();
        let tx_store = app.txs.clone();
        let mut txs = tx_store.write().await;
        txs.remove(&session_id);
        tracing::debug!(%session_id, "Closed session and cleaned up resources");
    });

    Ok(Sse::new(stream).keep_alive(KeepAlive::new().interval(ping_interval)))
}

pub struct SseServerTransport {
    stream: ReceiverStream<RxJsonRpcMessage<RoleServer>>,
    sink: PollSender<TxJsonRpcMessage<RoleServer>>,
    session_id: SessionId,
    tx_store: TxStore,
}

impl Sink<TxJsonRpcMessage<RoleServer>> for SseServerTransport {
    type Error = io::Error;

    fn poll_ready(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.sink
            .poll_ready_unpin(cx)
            .map_err(std::io::Error::other)
    }

    fn start_send(
        mut self: std::pin::Pin<&mut Self>,
        item: TxJsonRpcMessage<RoleServer>,
    ) -> Result<(), Self::Error> {
        self.sink
            .start_send_unpin(item)
            .map_err(std::io::Error::other)
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.sink
            .poll_flush_unpin(cx)
            .map_err(std::io::Error::other)
    }

    fn poll_close(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        let inner_close_result = self
            .sink
            .poll_close_unpin(cx)
            .map_err(std::io::Error::other);
        if inner_close_result.is_ready() {
            let session_id = self.session_id.clone();
            let tx_store = self.tx_store.clone();
            tokio::spawn(async move {
                tx_store.write().await.remove(&session_id);
            });
        }
        inner_close_result
    }
}

impl Stream for SseServerTransport {
    type Item = RxJsonRpcMessage<RoleServer>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        use futures::StreamExt;
        self.stream.poll_next_unpin(cx)
    }
}

#[derive(Debug)]
pub struct SseServer {
    transport_rx: tokio::sync::mpsc::UnboundedReceiver<SseServerTransport>,
    pub config: SseServerConfig,
}

impl SseServer {
    pub async fn serve(bind: SocketAddr) -> io::Result<Self> {
        Self::serve_with_config(SseServerConfig {
            bind,
            sse_path: "/sse".to_string(),
            post_path: "/message".to_string(),
            ct: CancellationToken::new(),
            sse_keep_alive: None,
        })
        .await
    }
    pub async fn serve_with_config(mut config: SseServerConfig) -> io::Result<Self> {
        let listener = tokio::net::TcpListener::bind(config.bind).await?;
        // Update config with actual bound address (important when port is 0)
        config.bind = listener.local_addr()?;
        let (sse_server, service) = Self::new(config);
        let ct = sse_server.config.ct.child_token();
        let server = axum::serve(listener, service).with_graceful_shutdown(async move {
            ct.cancelled().await;
            tracing::info!("sse server cancelled");
        });
        tokio::spawn(
            async move {
                if let Err(e) = server.await {
                    tracing::error!(error = %e, "sse server shutdown with error");
                }
            }
            .instrument(tracing::info_span!("sse-server", bind_address = %sse_server.config.bind)),
        );
        Ok(sse_server)
    }

    pub fn new(config: SseServerConfig) -> (SseServer, Router) {
        let (app, transport_rx) = App::new(
            config.post_path.clone(),
            config.sse_keep_alive.unwrap_or(DEFAULT_AUTO_PING_INTERVAL),
        );
        let router = Router::new()
            .route(&config.sse_path, get(sse_handler))
            .route(&config.post_path, post(post_event_handler))
            .with_state(app);

        let server = SseServer {
            transport_rx,
            config,
        };

        (server, router)
    }

    pub fn with_service<S, F>(mut self, service_provider: F) -> CancellationToken
    where
        S: Service<RoleServer>,
        F: Fn() -> S + Send + 'static,
    {
        use crate::service::ServiceExt;
        let ct = self.config.ct.clone();
        tokio::spawn(async move {
            while let Some(transport) = self.next_transport().await {
                let service = service_provider();
                let ct = self.config.ct.child_token();
                tokio::spawn(async move {
                    let server = service
                        .serve_with_ct(transport, ct)
                        .await
                        .map_err(std::io::Error::other)?;
                    server.waiting().await?;
                    tokio::io::Result::Ok(())
                });
            }
        });
        ct
    }

    /// This allows you to skip the initialization steps for incoming request.
    pub fn with_service_directly<S, F>(mut self, service_provider: F) -> CancellationToken
    where
        S: Service<RoleServer>,
        F: Fn() -> S + Send + 'static,
    {
        let ct = self.config.ct.clone();
        tokio::spawn(async move {
            while let Some(transport) = self.next_transport().await {
                let service = service_provider();
                let ct = self.config.ct.child_token();
                tokio::spawn(async move {
                    let server = serve_directly_with_ct(service, transport, None, ct);
                    server.waiting().await?;
                    tokio::io::Result::Ok(())
                });
            }
        });
        ct
    }

    pub fn cancel(&self) {
        self.config.ct.cancel();
    }

    pub async fn next_transport(&mut self) -> Option<SseServerTransport> {
        self.transport_rx.recv().await
    }
}

impl Stream for SseServer {
    type Item = SseServerTransport;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        self.transport_rx.poll_recv(cx)
    }
}

#[cfg(test)]
mod tests {
    use futures::{SinkExt, StreamExt};
    use tokio::time::timeout;

    use super::*;

    #[tokio::test]
    async fn test_session_management() {
        let (app, transport_rx) = App::new("/message".to_string(), Duration::from_secs(15));

        // Create a session
        let session_id = session_id();
        let (tx, _rx) = tokio::sync::mpsc::channel(64);

        // Insert session
        app.txs.write().await.insert(session_id.clone(), tx);

        // Verify session exists
        assert!(app.txs.read().await.contains_key(&session_id));

        // Remove session
        app.txs.write().await.remove(&session_id);

        // Verify session removed
        assert!(!app.txs.read().await.contains_key(&session_id));

        drop(transport_rx);
    }

    #[tokio::test]
    async fn test_sse_server_creation() {
        let config = SseServerConfig {
            bind: "127.0.0.1:0".parse().unwrap(),
            sse_path: "/sse".to_string(),
            post_path: "/message".to_string(),
            ct: CancellationToken::new(),
            sse_keep_alive: Some(Duration::from_secs(15)),
        };

        let (sse_server, router) = SseServer::new(config);

        assert_eq!(sse_server.config.sse_path, "/sse");
        assert_eq!(sse_server.config.post_path, "/message");

        // Router should be properly configured
        drop(router); // Just ensure it's created without panic
    }

    #[tokio::test]
    async fn test_transport_stream() {
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        let stream = ReceiverStream::new(rx);
        let (sink_tx, mut sink_rx) = tokio::sync::mpsc::channel(1);
        let sink = PollSender::new(sink_tx);

        let mut transport = SseServerTransport {
            stream,
            sink,
            session_id: session_id(),
            tx_store: Default::default(),
        };

        // Test sending through transport
        use crate::model::{EmptyResult, JsonRpcMessage, ServerResult};
        let msg: TxJsonRpcMessage<RoleServer> =
            JsonRpcMessage::Response(crate::model::JsonRpcResponse {
                jsonrpc: crate::model::JsonRpcVersion2_0,
                id: crate::model::NumberOrString::Number(1),
                result: ServerResult::EmptyResult(EmptyResult {}),
            });
        // For PollSender, we need to send through async context
        transport.send(msg).await.unwrap();

        // Should receive the message
        let received = timeout(Duration::from_millis(100), sink_rx.recv())
            .await
            .unwrap()
            .unwrap();

        match received {
            TxJsonRpcMessage::<RoleServer>::Response(_) => {}
            _ => panic!("Unexpected message type"),
        }

        // Test receiving through transport
        let client_msg: RxJsonRpcMessage<RoleServer> =
            crate::model::JsonRpcMessage::Notification(crate::model::JsonRpcNotification {
                jsonrpc: crate::model::JsonRpcVersion2_0,
                notification: crate::model::ClientNotification::CancelledNotification(
                    crate::model::Notification {
                        method: crate::model::CancelledNotificationMethod,
                        params: crate::model::CancelledNotificationParam {
                            request_id: crate::model::NumberOrString::Number(1),
                            reason: None,
                        },
                        extensions: Default::default(),
                    },
                ),
            });
        tx.send(client_msg).await.unwrap();
        drop(tx);

        let received = timeout(Duration::from_millis(100), transport.next())
            .await
            .unwrap()
            .unwrap();

        match received {
            RxJsonRpcMessage::<RoleServer>::Notification(_) => {}
            _ => panic!("Unexpected message type"),
        }
    }

    #[tokio::test]
    async fn test_post_event_handler_session_not_found() {
        use axum::{
            Json,
            extract::{Query, State},
            http::Request,
        };

        let (app, _) = App::new("/message".to_string(), Duration::from_secs(15));

        let query = PostEventQuery {
            session_id: "non-existent".to_string(),
        };

        // Create a minimal request parts
        let request = Request::builder()
            .method("POST")
            .uri("/message")
            .body(())
            .unwrap();
        let (parts, _) = request.into_parts();

        // Create a simple cancelled notification
        let client_msg = ClientJsonRpcMessage::Notification(crate::model::JsonRpcNotification {
            jsonrpc: crate::model::JsonRpcVersion2_0,
            notification: crate::model::ClientNotification::CancelledNotification(
                crate::model::Notification {
                    method: crate::model::CancelledNotificationMethod,
                    params: crate::model::CancelledNotificationParam {
                        request_id: crate::model::NumberOrString::Number(1),
                        reason: None,
                    },
                    extensions: Default::default(),
                },
            ),
        });

        let result = post_event_handler(State(app), Query(query), parts, Json(client_msg)).await;

        assert_eq!(result, Err(StatusCode::NOT_FOUND));
    }

    #[tokio::test]
    async fn test_server_with_cancellation() {
        let config = SseServerConfig {
            bind: "127.0.0.1:0".parse().unwrap(),
            sse_path: "/sse".to_string(),
            post_path: "/message".to_string(),
            ct: CancellationToken::new(),
            sse_keep_alive: None,
        };

        let ct_clone = config.ct.clone();
        let (mut sse_server, _) = SseServer::new(config);

        // Cancel immediately
        ct_clone.cancel();

        // next_transport should return None after cancellation
        let transport = timeout(Duration::from_millis(100), sse_server.next_transport()).await;
        assert!(transport.is_ok());
        assert!(transport.unwrap().is_none());
    }
}
