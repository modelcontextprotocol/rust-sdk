use std::{
    collections::HashMap,
    io,
    net::{Ipv6Addr, SocketAddr, SocketAddrV6},
    sync::Arc,
    time::Duration,
};

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode, request::Parts},
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::get,
};
use futures::Stream;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;
use tracing::Instrument;

use super::session::{EventId, SessionHandle, SessionWorker, StreamableHttpMessageReceiver};
use crate::{
    RoleServer, Service,
    model::ClientJsonRpcMessage,
    transport::common::{
        axum::{DEFAULT_AUTO_PING_INTERVAL, SessionId, session_id},
        http_header::{HEADER_LAST_EVENT_ID, HEADER_SESSION_ID},
    },
};
type SessionManager = Arc<tokio::sync::RwLock<HashMap<SessionId, SessionHandle>>>;

#[derive(Clone)]
struct App {
    session_manager: SessionManager,
    transport_tx: tokio::sync::mpsc::UnboundedSender<SessionWorker>,
    sse_ping_interval: Duration,
}

impl App {
    pub fn new(
        sse_ping_interval: Duration,
    ) -> (Self, tokio::sync::mpsc::UnboundedReceiver<SessionWorker>) {
        let (transport_tx, transport_rx) = tokio::sync::mpsc::unbounded_channel();
        (
            Self {
                session_manager: Default::default(),
                transport_tx,
                sse_ping_interval,
            },
            transport_rx,
        )
    }
}

fn receiver_as_stream(
    receiver: StreamableHttpMessageReceiver,
) -> impl Stream<Item = Result<Event, io::Error>> {
    use futures::StreamExt;
    ReceiverStream::new(receiver.inner).map(|message| {
        match serde_json::to_string(&message.message) {
            Ok(bytes) => Ok(Event::default()
                .event("message")
                .data(&bytes)
                .id(message.event_id.to_string())),
            Err(e) => Err(io::Error::new(io::ErrorKind::InvalidData, e)),
        }
    })
}

async fn post_handler(
    State(app): State<App>,
    parts: Parts,
    Json(mut message): Json<ClientJsonRpcMessage>,
) -> Result<Response, Response> {
    use futures::StreamExt;
    if let Some(session_id) = parts.headers.get(HEADER_SESSION_ID).cloned() {
        let session_id = session_id
            .to_str()
            .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()).into_response())?;
        tracing::debug!(session_id, ?message, "new client message");
        let handle = {
            let sm = app.session_manager.read().await;
            let session = sm
                .get(session_id)
                .ok_or((StatusCode::NOT_FOUND, "session not found").into_response())?;
            session.clone()
        };
        // inject request part
        message.insert_extension(parts);
        match &message {
            ClientJsonRpcMessage::Request(_) | ClientJsonRpcMessage::BatchRequest(_) => {
                let receiver = handle.establish_request_wise_channel().await.map_err(|e| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("fail to to establish request channel: {e}"),
                    )
                        .into_response()
                })?;
                let http_request_id = receiver.http_request_id;
                if let Err(push_err) = handle.push_message(message, http_request_id).await {
                    tracing::error!(session_id, ?push_err, "push message error");
                    return Err((
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("fail to push message: {push_err}"),
                    )
                        .into_response());
                }
                let stream =
                    ReceiverStream::new(receiver.inner).map(|message| match serde_json::to_string(
                        &message.message,
                    ) {
                        Ok(bytes) => Ok(Event::default()
                            .event("message")
                            .data(&bytes)
                            .id(message.event_id.to_string())),
                        Err(e) => Err(io::Error::new(io::ErrorKind::InvalidData, e)),
                    });
                Ok(Sse::new(stream)
                    .keep_alive(KeepAlive::new().interval(app.sse_ping_interval))
                    .into_response())
            }
            _ => {
                let result = handle.push_message(message, None).await;
                if result.is_err() {
                    Err((StatusCode::GONE, "session terminated").into_response())
                } else {
                    Ok(StatusCode::ACCEPTED.into_response())
                }
            }
        }
    } else {
        // expect initialize message
        let session_id = session_id();
        // inject request part
        message.insert_extension(parts);
        let (session, transport) =
            super::session::create_session(session_id.clone(), Default::default());
        let Ok(_) = app.transport_tx.send(transport) else {
            return Err((StatusCode::GONE, "session terminated").into_response());
        };

        let response = session.initialize(message).await.map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("fail to initialize: {e}"),
            )
                .into_response()
        })?;
        let mut response = Json(response).into_response();
        response.headers_mut().insert(
            HEADER_SESSION_ID,
            HeaderValue::from_bytes(session_id.as_bytes()).expect("should be valid header value"),
        );
        app.session_manager
            .write()
            .await
            .insert(session_id, session);
        Ok(response)
    }
}

async fn get_handler(
    State(app): State<App>,
    header_map: HeaderMap,
) -> Result<Sse<impl Stream<Item = Result<Event, io::Error>>>, Response> {
    let session_id = header_map
        .get(HEADER_SESSION_ID)
        .and_then(|v| v.to_str().ok());
    if let Some(session_id) = session_id {
        let last_event_id = header_map
            .get(HEADER_LAST_EVENT_ID)
            .and_then(|v| v.to_str().ok());
        let session = {
            let sm = app.session_manager.read().await;
            sm.get(session_id)
                .ok_or_else(|| {
                    (
                        StatusCode::NOT_FOUND,
                        format!("session {session_id} not found"),
                    )
                        .into_response()
                })?
                .clone()
        };
        match last_event_id {
            Some(last_event_id) => {
                let last_event_id = last_event_id.parse::<EventId>().map_err(|e| {
                    (StatusCode::BAD_REQUEST, format!("invalid event_id {e}")).into_response()
                })?;
                let receiver = session.resume(last_event_id).await.map_err(|e| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("resume error {e}"),
                    )
                        .into_response()
                })?;
                let stream = receiver_as_stream(receiver);
                Ok(Sse::new(stream).keep_alive(KeepAlive::new().interval(app.sse_ping_interval)))
            }
            None => {
                let receiver = session.establish_common_channel().await.map_err(|e| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("establish common channel error {e}"),
                    )
                        .into_response()
                })?;
                let stream = receiver_as_stream(receiver);
                Ok(Sse::new(stream).keep_alive(KeepAlive::new().interval(app.sse_ping_interval)))
            }
        }
    } else {
        Err((StatusCode::BAD_REQUEST, "missing session id").into_response())
    }
}

async fn delete_handler(
    State(app): State<App>,
    header_map: HeaderMap,
) -> Result<StatusCode, Response> {
    if let Some(session_id) = header_map.get(HEADER_SESSION_ID) {
        let session_id = session_id
            .to_str()
            .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()).into_response())?;
        let session = {
            let mut sm = app.session_manager.write().await;
            sm.remove(session_id)
                .ok_or((StatusCode::NOT_FOUND, "session not found").into_response())?
        };
        session.close().await.map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("fail to cancel session {session_id}: tokio join error: {e}"),
            )
                .into_response()
        })?;
        tracing::debug!(session_id, "session deleted");
        Ok(StatusCode::ACCEPTED)
    } else {
        Err((StatusCode::BAD_REQUEST, "missing session id").into_response())
    }
}

#[derive(Debug, Clone)]
pub struct StreamableHttpServerConfig {
    pub bind: SocketAddr,
    pub path: String,
    pub ct: CancellationToken,
    pub sse_keep_alive: Option<Duration>,
}
impl Default for StreamableHttpServerConfig {
    fn default() -> Self {
        Self {
            bind: SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::LOCALHOST, 80, 0, 0)),
            path: "/".to_string(),
            ct: CancellationToken::new(),
            sse_keep_alive: None,
        }
    }
}

#[derive(Debug)]
pub struct StreamableHttpServer {
    transport_rx: tokio::sync::mpsc::UnboundedReceiver<SessionWorker>,
    pub config: StreamableHttpServerConfig,
}

impl StreamableHttpServer {
    pub async fn serve(bind: SocketAddr) -> io::Result<Self> {
        Self::serve_with_config(StreamableHttpServerConfig {
            bind,
            ..Default::default()
        })
        .await
    }
    pub async fn serve_with_config(config: StreamableHttpServerConfig) -> io::Result<Self> {
        let (streamable_http_server, service) = Self::new(config);
        let listener = tokio::net::TcpListener::bind(streamable_http_server.config.bind).await?;
        let ct = streamable_http_server.config.ct.child_token();
        let server = axum::serve(listener, service).with_graceful_shutdown(async move {
            ct.cancelled().await;
            tracing::info!("streamable http server cancelled");
        });
        tokio::spawn(
            async move {
                if let Err(e) = server.await {
                    tracing::error!(error = %e, "streamable http server shutdown with error");
                }
            }
            .instrument(tracing::info_span!("streamable-http-server", bind_address = %streamable_http_server.config.bind)),
        );
        Ok(streamable_http_server)
    }

    /// Warning: This function creates a new StreamableHttpServer instance with the provided configuration.
    /// `App.post_path` may be incorrect if using `Router` as an embedded router.
    pub fn new(config: StreamableHttpServerConfig) -> (StreamableHttpServer, Router) {
        let (app, transport_rx) =
            App::new(config.sse_keep_alive.unwrap_or(DEFAULT_AUTO_PING_INTERVAL));
        let router = Router::new()
            .route(
                &config.path,
                get(get_handler).post(post_handler).delete(delete_handler),
            )
            .with_state(app);

        let server = StreamableHttpServer {
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
                    let server = service.serve_with_ct(transport, ct).await?;
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

    pub async fn next_transport(&mut self) -> Option<SessionWorker> {
        self.transport_rx.recv().await
    }
}
