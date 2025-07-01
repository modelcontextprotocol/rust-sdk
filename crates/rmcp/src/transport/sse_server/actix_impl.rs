use std::{collections::HashMap, io, sync::Arc, time::Duration};

use actix_web::{
    HttpRequest, HttpResponse, Result, Scope,
    error::ErrorInternalServerError,
    web::{self, Bytes, Data, Json, Query},
};
use futures::{Sink, SinkExt, Stream, StreamExt};
use tokio::sync::Mutex;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::{CancellationToken, PollSender};
use tracing::Instrument;

use crate::{
    RoleServer, Service,
    model::ClientJsonRpcMessage,
    service::{RxJsonRpcMessage, TxJsonRpcMessage, serve_directly_with_ct},
};

use super::common::{SseServerConfig, SessionId, session_id, DEFAULT_AUTO_PING_INTERVAL};
use crate::transport::common::http_header::HEADER_X_ACCEL_BUFFERING;

type TxStore =
    Arc<tokio::sync::RwLock<HashMap<SessionId, tokio::sync::mpsc::Sender<ClientJsonRpcMessage>>>>;

#[derive(Clone, Debug)]
struct AppData {
    txs: TxStore,
    transport_tx: tokio::sync::mpsc::UnboundedSender<SseServerTransport>,
    post_path: Arc<str>,
    sse_ping_interval: Duration,
}

impl AppData {
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
    app_data: Data<AppData>,
    query: Query<PostEventQuery>,
    _req: HttpRequest,
    message: Json<ClientJsonRpcMessage>,
) -> Result<HttpResponse> {
    let session_id = &query.session_id;
    tracing::debug!(session_id, ?message, "new client message");
    
    let tx = {
        let rg = app_data.txs.read().await;
        rg.get(session_id.as_str())
            .ok_or_else(|| actix_web::error::ErrorNotFound("Session not found"))?
            .clone()
    };
    
    // Note: In actix-web, we don't have direct access to modify extensions
    // This would need a different approach for passing HTTP request context
    
    if tx.send(message.0).await.is_err() {
        tracing::error!("send message error");
        return Err(actix_web::error::ErrorGone("Session closed"));
    }
    
    Ok(HttpResponse::Accepted().finish())
}

async fn sse_handler(
    app_data: Data<AppData>,
    _req: HttpRequest,
) -> Result<HttpResponse> {
    let session = session_id();
    tracing::info!(%session, "sse connection");
    
    let (from_client_tx, from_client_rx) = tokio::sync::mpsc::channel(64);
    let (to_client_tx, to_client_rx) = tokio::sync::mpsc::channel(64);
    let to_client_tx_clone = to_client_tx.clone();

    app_data.txs
        .write()
        .await
        .insert(session.clone(), from_client_tx);
    
    let _session_id = session.clone();
    let stream = ReceiverStream::new(from_client_rx);
    let sink = PollSender::new(to_client_tx);
    let transport = SseServerTransport {
        stream,
        sink,
        session_id: session.clone(),
        tx_store: app_data.txs.clone(),
    };
    
    let transport_send_result = app_data.transport_tx.send(transport);
    if transport_send_result.is_err() {
        tracing::warn!("send transport out error");
        return Err(ErrorInternalServerError("Failed to send transport, server is closed"));
    }
    
    let post_path = app_data.post_path.clone();
    let ping_interval = app_data.sse_ping_interval;
    let session_for_stream = session.clone();
    
    // Create SSE response stream
    let sse_stream = async_stream::stream! {
        // Send initial endpoint message
        yield Ok::<_, actix_web::Error>(Bytes::from(format!(
            "event: endpoint\ndata: {}?sessionId={}\n\n",
            post_path, session_for_stream
        )));
        
        // Set up ping interval
        let mut ping_interval = tokio::time::interval(ping_interval);
        ping_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        
        let mut rx = ReceiverStream::new(to_client_rx);
        
        loop {
            tokio::select! {
                Some(message) = rx.next() => {
                    match serde_json::to_string(&message) {
                        Ok(json) => {
                            yield Ok(Bytes::from(format!("event: message\ndata: {}\n\n", json)));
                        }
                        Err(e) => {
                            tracing::error!("Failed to serialize message: {}", e);
                        }
                    }
                }
                _ = ping_interval.tick() => {
                    yield Ok(Bytes::from(": ping\n\n"));
                }
                else => break,
            }
        }
    };
    
    // Clean up on disconnect
    let app_data_clone = app_data.clone();
    let session_for_cleanup = session.clone();
    actix_rt::spawn(async move {
        to_client_tx_clone.closed().await;
        
        let mut txs = app_data_clone.txs.write().await;
        txs.remove(&session_for_cleanup);
        tracing::debug!(%session_for_cleanup, "Closed session and cleaned up resources");
    });
    
    Ok(HttpResponse::Ok()
        .content_type("text/event-stream")
        .insert_header(("Cache-Control", "no-cache"))
        .insert_header((HEADER_X_ACCEL_BUFFERING, "no"))
        .streaming(sse_stream))
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
        self.stream.poll_next_unpin(cx)
    }
}

#[derive(Debug)]
pub struct SseServer {
    transport_rx: Arc<Mutex<tokio::sync::mpsc::UnboundedReceiver<SseServerTransport>>>,
    pub config: SseServerConfig,
    app_data: Data<AppData>,
}

impl SseServer {
    pub async fn serve(bind: std::net::SocketAddr) -> io::Result<Self> {
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
        let bind_addr = config.bind;
        let ct = config.ct.clone();
        
        // First bind to get the actual address
        let listener = std::net::TcpListener::bind(bind_addr)?;
        let actual_addr = listener.local_addr()?;
        listener.set_nonblocking(true)?;
        
        // Update config with actual address
        config.bind = actual_addr;
        let (sse_server, _) = Self::new(config);
        let app_data = sse_server.app_data.clone();
        let sse_path = sse_server.config.sse_path.clone();
        let post_path = sse_server.config.post_path.clone();
        
        let server = actix_web::HttpServer::new(move || {
            actix_web::App::new()
                .app_data(app_data.clone())
                .route(&sse_path, web::get().to(sse_handler))
                .route(&post_path, web::post().to(post_event_handler))
        })
        .listen(listener)?
        .run();
        
        let ct_child = ct.child_token();
        let server_handle = server.handle();
        
        actix_rt::spawn(async move {
            ct_child.cancelled().await;
            tracing::info!("sse server cancelled");
            server_handle.stop(true).await;
        });
        
        actix_rt::spawn(
            async move {
                if let Err(e) = server.await {
                    tracing::error!(error = %e, "sse server shutdown with error");
                }
            }
            .instrument(tracing::info_span!("sse-server", bind_address = %actual_addr)),
        );
        
        Ok(sse_server)
    }

    pub fn new(config: SseServerConfig) -> (SseServer, Scope) {
        let (app_data, transport_rx) = AppData::new(
            config.post_path.clone(),
            config.sse_keep_alive.unwrap_or(DEFAULT_AUTO_PING_INTERVAL),
        );
        
        let sse_path = config.sse_path.clone();
        let post_path = config.post_path.clone();
        
        let app_data = Data::new(app_data);
        
        let scope = web::scope("")
            .app_data(app_data.clone())
            .route(&sse_path, web::get().to(sse_handler))
            .route(&post_path, web::post().to(post_event_handler));
        
        let server = SseServer {
            transport_rx: Arc::new(Mutex::new(transport_rx)),
            config,
            app_data,
        };

        (server, scope)
    }

    pub fn with_service<S, F>(self, service_provider: F) -> CancellationToken
    where
        S: Service<RoleServer>,
        F: Fn() -> S + Send + 'static,
    {
        use crate::service::ServiceExt;
        let ct = self.config.ct.clone();
        let transport_rx = self.transport_rx.clone();
        
        actix_rt::spawn(async move {
            while let Some(transport) = transport_rx.lock().await.recv().await {
                let service = service_provider();
                let ct_child = ct.child_token();
                tokio::spawn(async move {
                    let server = service
                        .serve_with_ct(transport, ct_child)
                        .await
                        .map_err(std::io::Error::other)?;
                    server.waiting().await?;
                    tokio::io::Result::Ok(())
                });
            }
        });
        self.config.ct.clone()
    }

    /// This allows you to skip the initialization steps for incoming request.
    pub fn with_service_directly<S, F>(self, service_provider: F) -> CancellationToken
    where
        S: Service<RoleServer>,
        F: Fn() -> S + Send + 'static,
    {
        let ct = self.config.ct.clone();
        let transport_rx = self.transport_rx.clone();
        
        actix_rt::spawn(async move {
            while let Some(transport) = transport_rx.lock().await.recv().await {
                let service = service_provider();
                let ct_child = ct.child_token();
                tokio::spawn(async move {
                    let server = serve_directly_with_ct(service, transport, None, ct_child);
                    server.waiting().await?;
                    tokio::io::Result::Ok(())
                });
            }
        });
        self.config.ct.clone()
    }

    pub fn cancel(&self) {
        self.config.ct.cancel();
    }

    pub async fn next_transport(&self) -> Option<SseServerTransport> {
        self.transport_rx.lock().await.recv().await
    }
}

impl Stream for SseServer {
    type Item = SseServerTransport;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        let mut rx = match self.transport_rx.try_lock() {
            Ok(rx) => rx,
            Err(_) => {
                cx.waker().wake_by_ref();
                return std::task::Poll::Pending;
            }
        };
        rx.poll_recv(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::{SinkExt, StreamExt};
    use tokio::time::timeout;

    #[tokio::test]
    async fn test_session_management() {
        let (app_data, transport_rx) = AppData::new("/message".to_string(), Duration::from_secs(15));
        
        // Create a session
        let session_id = session_id();
        let (tx, _rx) = tokio::sync::mpsc::channel(64);
        
        // Insert session
        app_data.txs.write().await.insert(session_id.clone(), tx);
        
        // Verify session exists
        assert!(app_data.txs.read().await.contains_key(&session_id));
        
        // Remove session
        app_data.txs.write().await.remove(&session_id);
        
        // Verify session removed
        assert!(!app_data.txs.read().await.contains_key(&session_id));
        
        drop(transport_rx);
    }

    #[actix_web::test]
    async fn test_sse_server_creation() {
        let config = SseServerConfig {
            bind: "127.0.0.1:0".parse().unwrap(),
            sse_path: "/sse".to_string(),
            post_path: "/message".to_string(),
            ct: CancellationToken::new(),
            sse_keep_alive: Some(Duration::from_secs(15)),
        };
        
        let (sse_server, scope) = SseServer::new(config);
        
        assert_eq!(sse_server.config.sse_path, "/sse");
        assert_eq!(sse_server.config.post_path, "/message");
        
        // Scope should be properly configured
        drop(scope); // Just ensure it's created without panic
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
        use crate::model::{ServerResult, EmptyResult, JsonRpcMessage};
        let msg: TxJsonRpcMessage<RoleServer> = JsonRpcMessage::Response(crate::model::JsonRpcResponse {
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
            TxJsonRpcMessage::<RoleServer>::Response(_) => {},
            _ => panic!("Unexpected message type"),
        }
        
        // Test receiving through transport
        let client_msg: RxJsonRpcMessage<RoleServer> = crate::model::JsonRpcMessage::Notification(crate::model::JsonRpcNotification {
            jsonrpc: crate::model::JsonRpcVersion2_0,
            notification: crate::model::ClientNotification::CancelledNotification(
                crate::model::Notification {
                    method: crate::model::CancelledNotificationMethod,
                    params: crate::model::CancelledNotificationParam {
                        request_id: crate::model::NumberOrString::Number(1),
                        reason: None,
                    },
                    extensions: Default::default(),
                }
            ),
        });
        tx.send(client_msg).await.unwrap();
        drop(tx);
        
        let received = timeout(Duration::from_millis(100), transport.next())
            .await
            .unwrap()
            .unwrap();
        
        match received {
            RxJsonRpcMessage::<RoleServer>::Notification(_) => {},
            _ => panic!("Unexpected message type"),
        }
    }

    #[actix_web::test]
    async fn test_post_event_handler_session_not_found() {
        use actix_web::test;
        
        let (app_data, _) = AppData::new("/message".to_string(), Duration::from_secs(15));
        let app_data = Data::new(app_data);
        
        let query = PostEventQuery {
            session_id: "non-existent".to_string(),
        };
        
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
                }
            ),
        });
        
        let result = post_event_handler(
            app_data,
            Query(query),
            test::TestRequest::default().to_http_request(),
            Json(client_msg),
        ).await;
        
        assert!(result.is_err());
    }

    #[actix_web::test]
    async fn test_server_with_cancellation() {
        let config = SseServerConfig {
            bind: "127.0.0.1:0".parse().unwrap(),
            sse_path: "/sse".to_string(),
            post_path: "/message".to_string(),
            ct: CancellationToken::new(),
            sse_keep_alive: None,
        };
        
        let ct = config.ct.clone();
        let (sse_server, _) = SseServer::new(config);
        
        // Test that the cancellation token is properly connected
        assert!(!ct.is_cancelled());
        ct.cancel();
        assert!(ct.is_cancelled());
        
        // Verify server config
        assert!(sse_server.config.ct.is_cancelled());
    }

    #[actix_web::test]
    async fn test_sse_stream_generation() {
        let (app_data, mut transport_rx) = AppData::new("/message".to_string(), Duration::from_secs(15));
        let app_data = Data::new(app_data);
        
        // Call SSE handler
        let result = sse_handler(
            app_data.clone(),
            actix_web::test::TestRequest::default().to_http_request(),
        ).await;
        
        assert!(result.is_ok());
        let response = result.unwrap();
        
        // Check response headers
        assert_eq!(response.status(), actix_web::http::StatusCode::OK);
        assert_eq!(
            response.headers().get("content-type").unwrap(),
            "text/event-stream"
        );
        
        // Verify a transport was created
        let transport = transport_rx.try_recv();
        assert!(transport.is_ok());
    }
}