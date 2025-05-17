use std::{
    borrow::Cow,
    collections::{HashMap, HashSet, VecDeque},
    num::ParseIntError,
    sync::Arc,
};

use futures::{Sink, SinkExt, Stream};
use thiserror::Error;
use tokio::sync::{
    mpsc::{Receiver, Sender},
    oneshot,
};
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::{CancellationToken, DropGuard, PollSender};
use tracing::instrument;

use crate::{
    RoleServer,
    model::{
        CancelledNotificationParam, ClientJsonRpcMessage, ClientNotification, ClientRequest,
        JsonRpcNotification, JsonRpcRequest, Notification, ProgressNotificationParam,
        ProgressToken, RequestId, ServerJsonRpcMessage, ServerNotification,
    },
    transport::IntoTransport,
};

pub const HEADER_SESSION_ID: &str = "Mcp-Session-Id";
pub const HEADER_LAST_EVENT_ID: &str = "Last-Event-Id";
#[derive(Debug, Clone)]
pub struct ServerSessionMessage {
    pub event_id: EventId,
    pub message: Arc<ServerJsonRpcMessage>,
}

/// <index>-<request_id>
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EventId {
    http_request_id: Option<HttpRequestId>,
    index: usize,
}

impl std::fmt::Display for EventId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.index)?;
        match &self.http_request_id {
            Some(http_request_id) => write!(f, "/{http_request_id}"),
            None => write!(f, ""),
        }
    }
}

#[derive(Debug, Clone, Error)]
pub enum EventIdParseError {
    #[error("Invalid index: {0}")]
    InvalidIndex(ParseIntError),
    #[error("Invalid numeric request id: {0}")]
    InvalidNumericRequestId(ParseIntError),
    #[error("Missing request id type")]
    InvalidRequestIdType,
    #[error("Missing request id")]
    MissingRequestId,
}

impl std::str::FromStr for EventId {
    type Err = EventIdParseError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some((index, request_id)) = s.split_once("/") {
            let index = usize::from_str(index).map_err(EventIdParseError::InvalidIndex)?;
            let request_id = u64::from_str(request_id).map_err(EventIdParseError::InvalidIndex)?;
            Ok(EventId {
                http_request_id: Some(request_id),
                index,
            })
        } else {
            let index = usize::from_str(s).map_err(EventIdParseError::InvalidIndex)?;
            Ok(EventId {
                http_request_id: None,
                index,
            })
        }
    }
}

pub use crate::transport::common::axum::SessionId;

struct CachedTx {
    tx: Sender<ServerSessionMessage>,
    cache: VecDeque<ServerSessionMessage>,
    http_request_id: Option<HttpRequestId>,
    capacity: usize,
}

impl CachedTx {
    fn new(tx: Sender<ServerSessionMessage>, http_request_id: Option<HttpRequestId>) -> Self {
        Self {
            cache: VecDeque::with_capacity(tx.capacity()),
            capacity: tx.capacity(),
            tx,
            http_request_id,
        }
    }
    fn new_common(tx: Sender<ServerSessionMessage>) -> Self {
        Self::new(tx, None)
    }

    async fn send(&mut self, message: ServerJsonRpcMessage) {
        let index = self.cache.back().map_or(0, |m| m.event_id.index + 1);
        let event_id = EventId {
            http_request_id: self.http_request_id,
            index,
        };
        let message = ServerSessionMessage {
            event_id: event_id.clone(),
            message: Arc::new(message),
        };
        if self.cache.len() >= self.capacity {
            self.cache.pop_front();
            self.cache.push_back(message.clone());
        } else {
            self.cache.push_back(message.clone());
        }
        let _ = self.tx.send(message).await.inspect_err(|e| {
            let event_id = &e.0.event_id;
            tracing::trace!(%event_id, "trying to send message in a closed session")
        });
    }

    async fn sync(&mut self, index: usize) -> Result<(), SessionError> {
        let Some(front) = self.cache.front() else {
            return Ok(());
        };
        let sync_index = index.saturating_sub(front.event_id.index);
        if sync_index > self.cache.len() {
            // invalid index
            return Err(SessionError::InvalidEventId);
        }
        for message in self.cache.iter().skip(sync_index) {
            let send_result = self.tx.send(message.clone()).await;
            if send_result.is_err() {
                return Err(SessionError::ChannelClosed(
                    message.event_id.http_request_id,
                ));
            }
        }
        Ok(())
    }
}

struct HttpRequestWise {
    resources: HashSet<ResourceKey>,
    tx: CachedTx,
}

type HttpRequestId = u64;
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
enum ResourceKey {
    McpRequestId(RequestId),
    ProgressToken(ProgressToken),
}

struct SessionContext {
    id: SessionId,
    next_http_request_id: HttpRequestId,
    tx_router: HashMap<HttpRequestId, HttpRequestWise>,
    resource_router: HashMap<ResourceKey, HttpRequestId>,
    common: CachedTx,
    to_service_tx: Sender<ClientJsonRpcMessage>,
    event_rx: Receiver<SessionEvent>,
    session_config: SessionConfig,
}

#[derive(Debug, Error)]
pub enum SessionError {
    #[error("Invalid request id: {0}")]
    DuplicatedRequestId(HttpRequestId),
    #[error("Channel closed: {0:?}")]
    ChannelClosed(Option<HttpRequestId>),
    #[error("Cannot parse event id: {0}")]
    EventIdParseError(Cow<'static, str>),
    #[error("Session service terminated")]
    SessionServiceTerminated,
    #[error("Invalid event id")]
    InvalidEventId,
    #[error("Transport closed")]
    TransportClosed,
    #[error("IO error: {0}")]
    Io(std::io::Error),
}

impl From<SessionError> for std::io::Error {
    fn from(value: SessionError) -> Self {
        match value {
            SessionError::Io(io) => io,
            _ => std::io::Error::new(std::io::ErrorKind::Other, format!("Session error: {value}")),
        }
    }
}
impl From<std::io::Error> for SessionError {
    fn from(value: std::io::Error) -> Self {
        SessionError::Io(value)
    }
}

enum OutboundChannel {
    RequestWise { id: HttpRequestId, close: bool },
    Common,
}

pub struct StreamableHttpMessageReceiver {
    pub http_request_id: Option<HttpRequestId>,
    pub inner: Receiver<ServerSessionMessage>,
}

impl SessionContext {
    fn unregister_resource(&mut self, resource: &ResourceKey) {
        if let Some(http_request_id) = self.resource_router.remove(resource) {
            tracing::trace!(?resource, http_request_id, "unregister resource");
            if let Some(channel) = self.tx_router.get_mut(&http_request_id) {
                channel.resources.remove(resource);
                if channel.resources.is_empty() {
                    tracing::debug!(http_request_id, "close http request wise channel");
                    self.tx_router.remove(&http_request_id);
                }
            }
        }
    }
    fn register_resource(&mut self, resource: ResourceKey, http_request_id: HttpRequestId) {
        tracing::trace!(?resource, http_request_id, "register resource");
        if let Some(channel) = self.tx_router.get_mut(&http_request_id) {
            channel.resources.insert(resource.clone());
            self.resource_router.insert(resource, http_request_id);
        }
    }
    fn register_request(
        &mut self,
        request: &JsonRpcRequest<ClientRequest>,
        http_request_id: HttpRequestId,
    ) {
        use crate::model::GetMeta;
        self.register_resource(
            ResourceKey::McpRequestId(request.id.clone()),
            http_request_id,
        );
        if let Some(progress_token) = request.request.get_meta().get_progress_token() {
            self.register_resource(
                ResourceKey::ProgressToken(progress_token.clone()),
                http_request_id,
            );
        }
    }
    fn catch_cancellation_notification(
        &mut self,
        notification: &JsonRpcNotification<ClientNotification>,
    ) {
        if let ClientNotification::CancelledNotification(n) = &notification.notification {
            let request_id = n.params.request_id.clone();
            let resource = ResourceKey::McpRequestId(request_id);
            self.unregister_resource(&resource);
        }
    }
    fn next_http_request_id(&mut self) -> HttpRequestId {
        let id = self.next_http_request_id;
        self.next_http_request_id = self.next_http_request_id.wrapping_add(1);
        id
    }
    async fn send_to_service(&self, message: ClientJsonRpcMessage) -> Result<(), SessionError> {
        if self.to_service_tx.send(message).await.is_err() {
            return Err(SessionError::TransportClosed);
        }
        Ok(())
    }
    async fn establish_request_wise_channel(
        &mut self,
    ) -> Result<StreamableHttpMessageReceiver, SessionError> {
        let http_request_id = self.next_http_request_id();
        let (tx, rx) = tokio::sync::mpsc::channel(self.session_config.channel_capacity);
        self.tx_router.insert(
            http_request_id,
            HttpRequestWise {
                resources: Default::default(),
                tx: CachedTx::new(tx, Some(http_request_id)),
            },
        );
        tracing::debug!(http_request_id, "establish new request wise channel");
        Ok(StreamableHttpMessageReceiver {
            http_request_id: Some(http_request_id),
            inner: rx,
        })
    }
    fn resolve_outbound_channel(&self, message: &ServerJsonRpcMessage) -> OutboundChannel {
        match &message {
            ServerJsonRpcMessage::Request(_) => OutboundChannel::Common,
            ServerJsonRpcMessage::Notification(JsonRpcNotification {
                notification:
                    ServerNotification::ProgressNotification(Notification {
                        params: ProgressNotificationParam { progress_token, .. },
                        ..
                    }),
                ..
            }) => {
                let id = self
                    .resource_router
                    .get(&ResourceKey::ProgressToken(progress_token.clone()));

                if let Some(id) = id {
                    OutboundChannel::RequestWise {
                        id: *id,
                        close: false,
                    }
                } else {
                    OutboundChannel::Common
                }
            }
            ServerJsonRpcMessage::Notification(JsonRpcNotification {
                notification:
                    ServerNotification::CancelledNotification(Notification {
                        params: CancelledNotificationParam { request_id, .. },
                        ..
                    }),
                ..
            }) => {
                if let Some(id) = self
                    .resource_router
                    .get(&ResourceKey::McpRequestId(request_id.clone()))
                {
                    OutboundChannel::RequestWise {
                        id: *id,
                        close: false,
                    }
                } else {
                    OutboundChannel::Common
                }
            }
            ServerJsonRpcMessage::Notification(_) => OutboundChannel::Common,
            ServerJsonRpcMessage::Response(json_rpc_response) => {
                if let Some(id) = self
                    .resource_router
                    .get(&ResourceKey::McpRequestId(json_rpc_response.id.clone()))
                {
                    OutboundChannel::RequestWise {
                        id: *id,
                        close: false,
                    }
                } else {
                    OutboundChannel::Common
                }
            }
            ServerJsonRpcMessage::Error(json_rpc_error) => {
                if let Some(id) = self
                    .resource_router
                    .get(&ResourceKey::McpRequestId(json_rpc_error.id.clone()))
                {
                    OutboundChannel::RequestWise {
                        id: *id,
                        close: false,
                    }
                } else {
                    OutboundChannel::Common
                }
            }
            ServerJsonRpcMessage::BatchRequest(_) | ServerJsonRpcMessage::BatchResponse(_) => {
                // the server side should never yield a batch request or response now
                unreachable!("server side won't yield batch request or response")
            }
        }
    }
    async fn handle_server_message(
        &mut self,
        message: ServerJsonRpcMessage,
    ) -> Result<(), SessionError> {
        let outbound_channel = self.resolve_outbound_channel(&message);
        match outbound_channel {
            OutboundChannel::RequestWise { id, close } => {
                if let Some(request_wise) = self.tx_router.get_mut(&id) {
                    request_wise.tx.send(message).await;
                    if close {
                        self.tx_router.remove(&id);
                    }
                } else {
                    return Err(SessionError::ChannelClosed(Some(id)));
                }
            }
            OutboundChannel::Common => self.common.send(message).await,
        }
        Ok(())
    }
    async fn resume(
        &mut self,
        last_event_id: EventId,
    ) -> Result<StreamableHttpMessageReceiver, SessionError> {
        match last_event_id.http_request_id {
            Some(http_request_id) => {
                let request_wise = self
                    .tx_router
                    .get_mut(&http_request_id)
                    .ok_or(SessionError::ChannelClosed(Some(http_request_id)))?;
                let channel = tokio::sync::mpsc::channel(self.session_config.channel_capacity);
                let (tx, rx) = channel;
                request_wise.tx.tx = tx;
                let index = last_event_id.index;
                // sync messages after index
                request_wise.tx.sync(index).await?;
                Ok(StreamableHttpMessageReceiver {
                    http_request_id: Some(http_request_id),
                    inner: rx,
                })
            }
            None => {
                let channel = tokio::sync::mpsc::channel(self.session_config.channel_capacity);
                let (tx, rx) = channel;
                self.common.tx = tx;
                let index = last_event_id.index;
                // sync messages after index
                self.common.sync(index).await?;
                Ok(StreamableHttpMessageReceiver {
                    http_request_id: None,
                    inner: rx,
                })
            }
        }
    }
}

enum SessionEvent {
    ServiceMessage(ServerJsonRpcMessage),
    ClientMessage {
        message: ClientJsonRpcMessage,
        http_request_id: Option<HttpRequestId>,
    },
    EstablishRequestWiseChannel {
        responder: oneshot::Sender<Result<StreamableHttpMessageReceiver, SessionError>>,
    },
    CloseRequestWiseChannel {
        id: HttpRequestId,
        responder: oneshot::Sender<Result<(), SessionError>>,
    },
    Resume {
        last_event_id: EventId,
        responder: oneshot::Sender<Result<StreamableHttpMessageReceiver, SessionError>>,
    },
    InitializeRequest {
        request: ClientJsonRpcMessage,
        responder: oneshot::Sender<Result<ServerJsonRpcMessage, SessionError>>,
    },
}

#[derive(Debug, Clone)]
pub enum SessionQuitReason {
    ServiceTerminated,
    ClientTerminated,
    ExpectInitializeRequest,
    ExpectInitializeResponse,
    Cancelled,
}

impl SessionContext {
    #[instrument(name = "streamable_http_session", skip_all, fields(id = self.id.as_ref()))]
    pub async fn run(mut self, ct: CancellationToken) -> SessionQuitReason {
        // waiting for initialize request
        let Some(evt) = self.event_rx.recv().await else {
            return SessionQuitReason::ServiceTerminated;
        };
        let SessionEvent::InitializeRequest { request, responder } = evt else {
            return SessionQuitReason::ExpectInitializeRequest;
        };
        let send_result = self.send_to_service(request).await;
        if let Err(e) = send_result {
            let _ = responder.send(Err(e));
            return SessionQuitReason::ServiceTerminated;
        }
        let Some(evt) = self.event_rx.recv().await else {
            return SessionQuitReason::ServiceTerminated;
        };
        let SessionEvent::ServiceMessage(response) = evt else {
            return SessionQuitReason::ExpectInitializeResponse;
        };
        let response_result = responder.send(Ok(response));
        if response_result.is_err() {
            return SessionQuitReason::ClientTerminated;
        }
        let quit_reason = loop {
            let event = tokio::select! {
                event = self.event_rx.recv() => {
                    if let Some(event) = event {
                        event
                    } else {
                        break SessionQuitReason::ServiceTerminated;
                    }
                },

                _ = ct.cancelled() => {
                    break SessionQuitReason::Cancelled;
                }
            };
            match event {
                SessionEvent::ServiceMessage(json_rpc_message) => {
                    // catch response
                    match &json_rpc_message {
                        crate::model::JsonRpcMessage::Response(json_rpc_response) => {
                            let request_id = json_rpc_response.id.clone();
                            self.unregister_resource(&ResourceKey::McpRequestId(request_id));
                        }
                        crate::model::JsonRpcMessage::Error(json_rpc_error) => {
                            let request_id = json_rpc_error.id.clone();
                            self.unregister_resource(&ResourceKey::McpRequestId(request_id));
                        }
                        // unlikely happen
                        crate::model::JsonRpcMessage::BatchResponse(
                            json_rpc_batch_response_items,
                        ) => {
                            for item in json_rpc_batch_response_items {
                                let request_id = match item {
                                    crate::model::JsonRpcBatchResponseItem::Response(
                                        json_rpc_response,
                                    ) => json_rpc_response.id.clone(),
                                    crate::model::JsonRpcBatchResponseItem::Error(
                                        json_rpc_error,
                                    ) => json_rpc_error.id.clone(),
                                };
                                self.unregister_resource(&ResourceKey::McpRequestId(request_id));
                            }
                        }
                        _ => {
                            // no need to unregister resource
                        }
                    }
                    let _handle_result = self.handle_server_message(json_rpc_message).await;
                }
                SessionEvent::ClientMessage {
                    message: json_rpc_message,
                    http_request_id,
                } => {
                    match &json_rpc_message {
                        crate::model::JsonRpcMessage::Request(request) => {
                            if let Some(http_request_id) = http_request_id {
                                self.register_request(request, http_request_id)
                            }
                        }
                        crate::model::JsonRpcMessage::Notification(notification) => {
                            self.catch_cancellation_notification(notification)
                        }
                        crate::model::JsonRpcMessage::BatchRequest(items) => {
                            for r in items {
                                match r {
                                    crate::model::JsonRpcBatchRequestItem::Request(request) => {
                                        if let Some(http_request_id) = http_request_id {
                                            self.register_request(request, http_request_id)
                                        }
                                    }
                                    crate::model::JsonRpcBatchRequestItem::Notification(
                                        notification,
                                    ) => self.catch_cancellation_notification(notification),
                                }
                            }
                        }
                        _ => {}
                    }
                    let _handle_result = self.send_to_service(json_rpc_message).await;
                }
                SessionEvent::EstablishRequestWiseChannel { responder } => {
                    let handle_result = self.establish_request_wise_channel().await;
                    let _ = responder.send(handle_result);
                }
                SessionEvent::CloseRequestWiseChannel { id, responder } => {
                    let _handle_result = self.tx_router.remove(&id);
                    let _ = responder.send(Ok(()));
                }
                SessionEvent::Resume {
                    last_event_id,
                    responder,
                } => {
                    let handle_result = self.resume(last_event_id).await;
                    let _ = responder.send(handle_result);
                }
                _ => {
                    // ignore
                }
            }
        };
        tracing::debug!("session terminated: {:?}", quit_reason);
        quit_reason
    }
}

pub struct Session {
    handle: SessionHandle,
    guard: DropGuard,
    task_handle: tokio::task::JoinHandle<SessionQuitReason>,
}

impl Session {
    pub fn handle(&self) -> &SessionHandle {
        &self.handle
    }
    pub async fn cancel(self) -> Result<SessionQuitReason, tokio::task::JoinError> {
        self.guard.disarm().cancel();
        self.task_handle.await
    }
}

#[derive(Debug, Clone)]
pub struct SessionHandle {
    // after all event_tx drop, inner task will be terminated
    event_tx: Sender<SessionEvent>,
}

impl SessionHandle {
    pub async fn push_message(
        &self,
        message: ClientJsonRpcMessage,
        http_request_id: Option<HttpRequestId>,
    ) -> Result<(), SessionError> {
        self.event_tx
            .send(SessionEvent::ClientMessage {
                message,
                http_request_id,
            })
            .await
            .map_err(|_| SessionError::SessionServiceTerminated)?;
        Ok(())
    }

    pub async fn establish_request_wise_channel(
        &self,
    ) -> Result<StreamableHttpMessageReceiver, SessionError> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.event_tx
            .send(SessionEvent::EstablishRequestWiseChannel { responder: tx })
            .await
            .map_err(|_| SessionError::SessionServiceTerminated)?;
        rx.await
            .map_err(|_| SessionError::SessionServiceTerminated)?
    }
    pub async fn close_request_wise_channel(
        &self,
        request_id: HttpRequestId,
    ) -> Result<(), SessionError> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.event_tx
            .send(SessionEvent::CloseRequestWiseChannel {
                id: request_id,
                responder: tx,
            })
            .await
            .map_err(|_| SessionError::SessionServiceTerminated)?;
        rx.await
            .map_err(|_| SessionError::SessionServiceTerminated)?
    }
    pub async fn establish_common_channel(
        &self,
    ) -> Result<StreamableHttpMessageReceiver, SessionError> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.event_tx
            .send(SessionEvent::Resume {
                last_event_id: EventId {
                    http_request_id: None,
                    index: 0,
                },
                responder: tx,
            })
            .await
            .map_err(|_| SessionError::SessionServiceTerminated)?;
        rx.await
            .map_err(|_| SessionError::SessionServiceTerminated)?
    }

    pub async fn resume(
        &self,
        last_event_id: EventId,
    ) -> Result<StreamableHttpMessageReceiver, SessionError> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.event_tx
            .send(SessionEvent::Resume {
                last_event_id,
                responder: tx,
            })
            .await
            .map_err(|_| SessionError::SessionServiceTerminated)?;
        rx.await
            .map_err(|_| SessionError::SessionServiceTerminated)?
    }

    pub async fn initialize(
        &self,
        request: ClientJsonRpcMessage,
    ) -> Result<ServerJsonRpcMessage, SessionError> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.event_tx
            .send(SessionEvent::InitializeRequest {
                request,
                responder: tx,
            })
            .await
            .map_err(|_| SessionError::SessionServiceTerminated)?;
        rx.await
            .map_err(|_| SessionError::SessionServiceTerminated)?
    }
}

pub struct SessionTransport {
    session_handle: SessionHandle,
    to_service_rx: Receiver<ClientJsonRpcMessage>,
}

impl IntoTransport<RoleServer, SessionError, ()> for SessionTransport {
    fn into_transport(
        self,
    ) -> (
        impl Sink<ServerJsonRpcMessage, Error = SessionError> + Send + 'static,
        impl Stream<Item = ClientJsonRpcMessage> + Send + 'static,
    ) {
        let stream = ReceiverStream::new(self.to_service_rx);
        let sink = PollSender::new(self.session_handle.event_tx.clone())
            .sink_map_err(|_| SessionError::SessionServiceTerminated)
            .with(async |m| Ok(SessionEvent::ServiceMessage(m)));
        (sink, stream)
    }
}

#[derive(Debug, Clone)]
pub struct SessionConfig {
    channel_capacity: usize,
}

impl SessionConfig {
    pub const DEFAULT_CHANNEL_CAPACITY: usize = 16;
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            channel_capacity: Self::DEFAULT_CHANNEL_CAPACITY,
        }
    }
}

pub fn create_session(id: SessionId, config: SessionConfig) -> (Session, SessionTransport) {
    let (to_service_tx, to_service_rx) = tokio::sync::mpsc::channel(config.channel_capacity);
    let (event_tx, event_rx) = tokio::sync::mpsc::channel(config.channel_capacity);
    let (common_tx, _) = tokio::sync::mpsc::channel(config.channel_capacity);
    let common = CachedTx::new_common(common_tx);
    tracing::info!(session_id = ?id, "create new session");
    let session_context = SessionContext {
        next_http_request_id: 0,
        id,
        tx_router: HashMap::new(),
        resource_router: HashMap::new(),
        common,
        to_service_tx,
        event_rx,
        session_config: config.clone(),
    };
    let ct = CancellationToken::new();
    let handle = SessionHandle { event_tx };
    let task_handle = tokio::spawn(session_context.run(ct.child_token()));
    let session = Session {
        handle: handle.clone(),
        task_handle,
        guard: ct.drop_guard(),
    };
    let session_transport = SessionTransport {
        to_service_rx,
        session_handle: handle,
    };
    (session, session_transport)
}
