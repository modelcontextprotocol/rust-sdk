use std::{
    borrow::Cow,
    collections::{HashMap, HashSet, VecDeque},
    num::ParseIntError,
    sync::Arc,
};

use thiserror::Error;
use tokio::sync::{
    mpsc::{Receiver, Sender},
    oneshot,
};
use tracing::instrument;

use crate::{
    RoleServer,
    model::{
        CancelledNotificationParam, ClientJsonRpcMessage, ClientNotification, ClientRequest,
        JsonRpcNotification, JsonRpcRequest, Notification, ProgressNotificationParam,
        ProgressToken, RequestId, ServerJsonRpcMessage, ServerNotification,
    },
    transport::{
        WorkerTransport,
        worker::{Worker, WorkerContext, WorkerQuitReason, WorkerSendRequest},
    },
};

#[derive(Debug, Clone)]
pub struct ServerSessionMessage {
    pub event_id: EventId,
    pub message: Arc<ServerJsonRpcMessage>,
}

/// `<index>/request_id>`
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

pub struct SessionWorker {
    id: SessionId,
    next_http_request_id: HttpRequestId,
    tx_router: HashMap<HttpRequestId, HttpRequestWise>,
    resource_router: HashMap<ResourceKey, HttpRequestId>,
    common: CachedTx,
    event_rx: Receiver<SessionEvent>,
    session_config: SessionConfig,
}

impl SessionWorker {
    pub fn id(&self) -> &SessionId {
        &self.id
    }
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
    Io(#[from] std::io::Error),
    #[error("Tokio join error {0}")]
    TokioJoinError(#[from] tokio::task::JoinError),
}

impl From<SessionError> for std::io::Error {
    fn from(value: SessionError) -> Self {
        match value {
            SessionError::Io(io) => io,
            _ => std::io::Error::new(std::io::ErrorKind::Other, format!("Session error: {value}")),
        }
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

impl SessionWorker {
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
    Close,
}

#[derive(Debug, Clone)]
pub enum SessionQuitReason {
    ServiceTerminated,
    ClientTerminated,
    ExpectInitializeRequest,
    ExpectInitializeResponse,
    Cancelled,
}

#[derive(Debug, Clone)]
pub struct SessionHandle {
    id: SessionId,
    // after all event_tx drop, inner task will be terminated
    event_tx: Sender<SessionEvent>,
}

impl SessionHandle {
    /// Get the session id
    pub fn id(&self) -> &SessionId {
        &self.id
    }

    /// Close the session
    pub async fn close(&self) -> Result<(), SessionError> {
        self.event_tx
            .send(SessionEvent::Close)
            .await
            .map_err(|_| SessionError::SessionServiceTerminated)?;
        Ok(())
    }

    /// Send a message to the session
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

    /// establish a channel for a http-request, the corresponded message from server will be
    /// sent through this channel. The channel will be closed when the request is completed,
    /// or you can close it manually by calling [`SessionHandle::close_request_wise_channel`].
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

    /// close the http-request wise channel.
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

    /// Establish a common channel for general purpose messages.
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

    /// Resume streaming response by the last event id. This is suitable for both request wise and common channel.
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

    /// Send an initialize request to the session. And wait for the initialized response.
    ///
    /// This is used to establish a session with the server.
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

pub type SessionTransport = WorkerTransport<SessionWorker>;

impl Worker for SessionWorker {
    type Error = SessionError;
    type Role = RoleServer;
    fn err_closed() -> Self::Error {
        SessionError::TransportClosed
    }
    fn err_join(e: tokio::task::JoinError) -> Self::Error {
        SessionError::TokioJoinError(e)
    }
    fn config(&self) -> crate::transport::worker::WorkerConfig {
        crate::transport::worker::WorkerConfig {
            name: Some(format!("streamable-http-session-{}", self.id)),
            channel_buffer_capacity: self.session_config.channel_capacity,
        }
    }
    #[instrument(name = "streamable_http_session", skip_all, fields(id = self.id.as_ref()))]
    async fn run(mut self, mut context: WorkerContext<Self>) -> Result<(), WorkerQuitReason> {
        enum InnerEvent {
            FromHttpService(SessionEvent),
            FromHandler(WorkerSendRequest<SessionWorker>),
        }
        // waiting for initialize request
        let evt = self.event_rx.recv().await.ok_or_else(|| {
            WorkerQuitReason::fatal("transport terminated", "get initialize request")
        })?;
        let SessionEvent::InitializeRequest { request, responder } = evt else {
            return Err(WorkerQuitReason::fatal(
                "unexpected message",
                "get initialize request",
            ));
        };
        context.send_to_handler(request).await?;
        let send_initialize_response = context.recv_from_handler().await?;
        responder
            .send(Ok(send_initialize_response.message))
            .map_err(|_| {
                WorkerQuitReason::fatal(
                    "failed to send initialize response to http service",
                    "send initialize response",
                )
            })?;
        send_initialize_response
            .responder
            .send(Ok(()))
            .map_err(|_| WorkerQuitReason::HandlerTerminated)?;
        let ct = context.cancellation_token.clone();
        loop {
            let event = tokio::select! {
                event = self.event_rx.recv() => {
                    if let Some(event) = event {
                        InnerEvent::FromHttpService(event)
                    } else {
                        return Err(WorkerQuitReason::fatal("session dropped", "waiting next session event"))
                    }
                },
                from_handler = context.recv_from_handler() => {
                    InnerEvent::FromHandler(from_handler?)
                }
                _ = ct.cancelled() => {
                    return Err(WorkerQuitReason::Cancelled)
                }
            };
            match event {
                InnerEvent::FromHandler(WorkerSendRequest { message, responder }) => {
                    // catch response
                    match &message {
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
                    let handle_result = self.handle_server_message(message).await;
                    let _ = responder.send(handle_result);
                }
                InnerEvent::FromHttpService(SessionEvent::ClientMessage {
                    message: json_rpc_message,
                    http_request_id,
                }) => {
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
                    context.send_to_handler(json_rpc_message).await?;
                }
                InnerEvent::FromHttpService(SessionEvent::EstablishRequestWiseChannel {
                    responder,
                }) => {
                    let handle_result = self.establish_request_wise_channel().await;
                    let _ = responder.send(handle_result);
                }
                InnerEvent::FromHttpService(SessionEvent::CloseRequestWiseChannel {
                    id,
                    responder,
                }) => {
                    let _handle_result = self.tx_router.remove(&id);
                    let _ = responder.send(Ok(()));
                }
                InnerEvent::FromHttpService(SessionEvent::Resume {
                    last_event_id,
                    responder,
                }) => {
                    let handle_result = self.resume(last_event_id).await;
                    let _ = responder.send(handle_result);
                }
                InnerEvent::FromHttpService(SessionEvent::Close) => {
                    return Err(WorkerQuitReason::TransportClosed);
                }
                _ => {
                    // ignore
                }
            }
        }
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

/// Create a new session with the given id and configuration.
///
/// This function will return a pair of [`SessionHandle`] and [`SessionWorker`].
///
/// You can run the [`SessionWorker`] as a transport for mcp server. And use the [`SessionHandle`] operate the session.
pub fn create_session(
    id: impl Into<SessionId>,
    config: SessionConfig,
) -> (SessionHandle, SessionWorker) {
    let id = id.into();
    let (event_tx, event_rx) = tokio::sync::mpsc::channel(config.channel_capacity);
    let (common_tx, _) = tokio::sync::mpsc::channel(config.channel_capacity);
    let common = CachedTx::new_common(common_tx);
    tracing::info!(session_id = ?id, "create new session");
    let handle = SessionHandle {
        event_tx,
        id: id.clone(),
    };
    let session_worker = SessionWorker {
        next_http_request_id: 0,
        id,
        tx_router: HashMap::new(),
        resource_router: HashMap::new(),
        common,
        event_rx,
        session_config: config.clone(),
    };
    (handle, session_worker)
}
