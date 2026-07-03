use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use futures::{Stream, StreamExt, future::BoxFuture, stream::BoxStream};
use http::{HeaderName, HeaderValue};
pub use sse_stream::Error as SseError;
use sse_stream::Sse;
use thiserror::Error;
use tokio_util::sync::CancellationToken;
use tracing::debug;

use super::common::client_side_sse::{ExponentialBackoff, SseRetryPolicy, SseStreamReconnect};
use crate::{
    RoleClient,
    model::{
        ClientJsonRpcMessage, ClientNotification, ErrorData, InitializedNotification, JsonObject,
        ProtocolVersion, RequestId, ServerJsonRpcMessage, ServerResult,
    },
    transport::{
        common::{client_side_sse::SseAutoReconnectStream, mcp_headers},
        worker::{Worker, WorkerQuitReason, WorkerSendRequest, WorkerTransport},
    },
};

type BoxedSseStream = BoxStream<'static, Result<Sse, SseError>>;

fn build_request_headers(
    base: &HashMap<HeaderName, HeaderValue>,
    message: &ClientJsonRpcMessage,
    tool_cache: &HashMap<String, Arc<JsonObject>>,
    version: &ProtocolVersion,
) -> HashMap<HeaderName, HeaderValue> {
    use serde_json::Value;

    let mut headers = base.clone();
    if *version >= ProtocolVersion::STANDARD_HEADERS {
        if let Ok(value) = serde_json::to_value(message) {
            let schema = value
                .get("method")
                .and_then(Value::as_str)
                .filter(|method| *method == "tools/call")
                .and_then(|_| value.get("params"))
                .and_then(|params| params.get("name"))
                .and_then(Value::as_str)
                .and_then(|name| tool_cache.get(name))
                .map(Arc::as_ref);
            for (name, val) in mcp_headers::standard_request_headers(&value, schema) {
                headers.insert(name, val);
            }
        }
    }
    headers
}

fn cache_tools_from_response(
    cache: &mut HashMap<String, Arc<JsonObject>>,
    message: &ServerJsonRpcMessage,
) {
    if let ServerJsonRpcMessage::Response(response) = message {
        if let ServerResult::ListToolsResult(list) = &response.result {
            for tool in &list.tools {
                if let Err(reason) =
                    mcp_headers::validate_param_header_annotations(&tool.input_schema)
                {
                    tracing::warn!(tool = %tool.name, "ignoring x-mcp-header annotations: {reason}");
                    continue;
                }
                cache.insert(tool.name.to_string(), tool.input_schema.clone());
            }
        }
    }
}

fn negotiate_version_headers(
    init_response: &ServerJsonRpcMessage,
    base: HashMap<HeaderName, HeaderValue>,
) -> (ProtocolVersion, HashMap<HeaderName, HeaderValue>) {
    let mut version = ProtocolVersion::default();
    let mut headers = base;
    if let ServerJsonRpcMessage::Response(response) = init_response {
        if let ServerResult::InitializeResult(init_result) = &response.result {
            version = init_result.protocol_version.clone();
            // HeaderName::from_static requires lowercase
            if let Ok(hv) = HeaderValue::from_str(init_result.protocol_version.as_str()) {
                headers.insert(HeaderName::from_static("mcp-protocol-version"), hv);
            }
        }
    }
    (version, headers)
}

#[derive(Debug)]
#[non_exhaustive]
pub struct AuthRequiredError {
    pub www_authenticate_header: String,
}

impl AuthRequiredError {
    /// Create a new `AuthRequiredError` instance.
    pub fn new(www_authenticate_header: String) -> Self {
        Self {
            www_authenticate_header,
        }
    }
}

#[derive(Debug)]
#[non_exhaustive]
pub struct InsufficientScopeError {
    pub www_authenticate_header: String,
    pub required_scope: Option<String>,
}

impl InsufficientScopeError {
    /// Create a new `InsufficientScopeError` instance.
    pub fn new(www_authenticate_header: String, required_scope: Option<String>) -> Self {
        Self {
            www_authenticate_header,
            required_scope,
        }
    }

    /// check if scope upgrade is possible (i.e., we know what scope is required)
    pub fn can_upgrade(&self) -> bool {
        self.required_scope.is_some()
    }

    /// get the required scope for upgrade
    pub fn get_required_scope(&self) -> Option<&str> {
        self.required_scope.as_deref()
    }
}

#[derive(Error, Debug)]
#[non_exhaustive]
pub enum StreamableHttpError<E: std::error::Error + Send + Sync + 'static> {
    #[error("SSE error: {0}")]
    Sse(#[from] SseError),
    #[error("Io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Client error: {0}")]
    Client(E),
    #[error("unexpected end of stream")]
    UnexpectedEndOfStream,
    #[error("unexpected server response: {0}")]
    UnexpectedServerResponse(Cow<'static, str>),
    #[error("Unexpected content type: {0:?}")]
    UnexpectedContentType(Option<String>),
    #[error("Server does not support SSE")]
    ServerDoesNotSupportSse,
    #[error("Server does not support delete session")]
    ServerDoesNotSupportDeleteSession,
    #[error("Tokio join error: {0}")]
    TokioJoinError(#[from] tokio::task::JoinError),
    #[error("Deserialize error: {0}")]
    Deserialize(#[from] serde_json::Error),
    #[error("Transport channel closed")]
    TransportChannelClosed,
    #[error("Missing session id in HTTP response")]
    MissingSessionIdInResponse,
    #[cfg(feature = "auth")]
    #[error("Auth error: {0}")]
    Auth(#[from] crate::transport::auth::AuthError),
    #[error("Auth required")]
    AuthRequired(AuthRequiredError),
    #[error("Insufficient scope")]
    InsufficientScope(InsufficientScopeError),
    #[error("Header name '{0}' is reserved and conflicts with default headers")]
    ReservedHeaderConflict(String),
    #[error("Session expired (HTTP 404)")]
    SessionExpired,
}

#[derive(Debug, Clone, Error)]
#[non_exhaustive]
pub enum StreamableHttpProtocolError {
    #[error("Missing session id in response")]
    MissingSessionIdInResponse,
}

#[allow(clippy::large_enum_variant)]
#[non_exhaustive]
pub enum StreamableHttpPostResponse {
    Accepted,
    Json(ServerJsonRpcMessage, Option<String>),
    Sse(BoxedSseStream, Option<String>),
}

impl std::fmt::Debug for StreamableHttpPostResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Accepted => write!(f, "Accepted"),
            Self::Json(arg0, arg1) => f.debug_tuple("Json").field(arg0).field(arg1).finish(),
            Self::Sse(_, arg1) => f.debug_tuple("Sse").field(arg1).finish(),
        }
    }
}

impl StreamableHttpPostResponse {
    pub async fn expect_initialized<E>(
        self,
    ) -> Result<(ServerJsonRpcMessage, Option<String>), StreamableHttpError<E>>
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        match self {
            Self::Json(message, session_id) => Ok((message, session_id)),
            Self::Sse(mut stream, session_id) => {
                while let Some(event) = stream.next().await {
                    let event = event?;
                    let payload = event.data.unwrap_or_default();
                    if payload.trim().is_empty() {
                        continue;
                    }

                    let message: ServerJsonRpcMessage = serde_json::from_str(&payload)?;

                    if matches!(message, ServerJsonRpcMessage::Response(_)) {
                        return Ok((message, session_id));
                    }

                    debug!(
                        ?message,
                        "received message before initialize response; continuing to drain stream"
                    );
                }

                Err(StreamableHttpError::UnexpectedServerResponse(
                    "empty sse stream".into(),
                ))
            }
            _ => Err(StreamableHttpError::UnexpectedServerResponse(
                "expect initialized, accepted".into(),
            )),
        }
    }

    pub fn expect_json<E>(self) -> Result<ServerJsonRpcMessage, StreamableHttpError<E>>
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        match self {
            Self::Json(message, ..) => Ok(message),
            got => Err(StreamableHttpError::UnexpectedServerResponse(
                format!("expect json, got {got:?}").into(),
            )),
        }
    }

    pub fn expect_accepted_or_json<E>(self) -> Result<(), StreamableHttpError<E>>
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        match self {
            Self::Accepted => Ok(()),
            // Tolerate servers that return 200 with JSON for notifications
            Self::Json(..) => Ok(()),
            got => Err(StreamableHttpError::UnexpectedServerResponse(
                format!("expect accepted or json, got {got:?}").into(),
            )),
        }
    }
}

pub trait StreamableHttpClient: Clone + Send + 'static {
    type Error: std::error::Error + Send + Sync + 'static;
    fn post_message(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> impl Future<Output = Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>>>
    + Send
    + '_;
    fn delete_session(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> impl Future<Output = Result<(), StreamableHttpError<Self::Error>>> + Send + '_;
    fn get_stream(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        last_event_id: Option<String>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> impl Future<
        Output = Result<
            BoxStream<'static, Result<Sse, SseError>>,
            StreamableHttpError<Self::Error>,
        >,
    > + Send
    + '_;
}

#[non_exhaustive]
pub struct RetryConfig {
    pub max_times: Option<usize>,
    pub min_duration: Duration,
}

struct StreamableHttpClientReconnect<C> {
    pub client: C,
    pub session_id: Arc<str>,
    pub uri: Arc<str>,
    pub auth_header: Option<String>,
    pub custom_headers: HashMap<HeaderName, HeaderValue>,
}

impl<C: StreamableHttpClient> SseStreamReconnect for StreamableHttpClientReconnect<C> {
    type Error = StreamableHttpError<C::Error>;
    type Future = BoxFuture<'static, Result<BoxedSseStream, Self::Error>>;
    fn retry_connection(&mut self, last_event_id: Option<&str>) -> Self::Future {
        let client = self.client.clone();
        let uri = self.uri.clone();
        let session_id = self.session_id.clone();
        let auth_header = self.auth_header.clone();
        let custom_headers = self.custom_headers.clone();
        let last_event_id = last_event_id.map(|s| s.to_owned());
        Box::pin(async move {
            client
                .get_stream(uri, session_id, last_event_id, auth_header, custom_headers)
                .await
        })
    }
}

/// Info retained for cleaning up the session when the worker exits.
struct SessionCleanupInfo<C> {
    client: C,
    uri: Arc<str>,
    session_id: Arc<str>,
    auth_header: Option<String>,
    protocol_headers: HashMap<HeaderName, HeaderValue>,
}

#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct StreamableHttpClientWorker<C: StreamableHttpClient> {
    pub client: C,
    pub config: StreamableHttpClientTransportConfig,
}

impl<C: StreamableHttpClient + Default> StreamableHttpClientWorker<C> {
    pub fn new_simple(url: impl Into<Arc<str>>) -> Self {
        Self {
            client: C::default(),
            config: StreamableHttpClientTransportConfig {
                uri: url.into(),
                ..Default::default()
            },
        }
    }
}

impl<C: StreamableHttpClient> StreamableHttpClientWorker<C> {
    pub fn new(client: C, config: StreamableHttpClientTransportConfig) -> Self {
        Self { client, config }
    }
}

impl<C: StreamableHttpClient> StreamableHttpClientWorker<C> {
    fn client_request_id(message: &ClientJsonRpcMessage) -> Option<RequestId> {
        match message {
            ClientJsonRpcMessage::Request(request) => Some(request.id.clone()),
            _ => None,
        }
    }

    fn server_response_id(message: &ServerJsonRpcMessage) -> Option<&RequestId> {
        match message {
            ServerJsonRpcMessage::Response(response) => Some(&response.id),
            ServerJsonRpcMessage::Error(error) => error.id.as_ref(),
            _ => None,
        }
    }

    fn mark_stream_response_pending(
        pending_stream_response_ids: &mut HashSet<RequestId>,
        request_id: Option<RequestId>,
    ) {
        if let Some(request_id) = request_id {
            pending_stream_response_ids.insert(request_id);
        }
    }

    fn clear_stream_response_pending(
        pending_stream_response_ids: &mut HashSet<RequestId>,
        message: &ServerJsonRpcMessage,
    ) {
        if let Some(id) = Self::server_response_id(message) {
            pending_stream_response_ids.remove(id);
        }
    }

    async fn drain_queued_stream_messages(
        sse_worker_rx: &mut tokio::sync::mpsc::Receiver<ServerJsonRpcMessage>,
        context: &mut super::worker::WorkerContext<Self>,
        pending_stream_response_ids: &mut HashSet<RequestId>,
    ) -> Result<(), WorkerQuitReason<StreamableHttpError<C::Error>>> {
        loop {
            match sse_worker_rx.try_recv() {
                Ok(message) => {
                    Self::clear_stream_response_pending(pending_stream_response_ids, &message);
                    context.send_to_handler(message).await?;
                }
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => return Ok(()),
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => return Ok(()),
            }
        }
    }

    async fn fail_pending_stream_responses(
        context: &mut super::worker::WorkerContext<Self>,
        pending_stream_response_ids: &mut HashSet<RequestId>,
    ) -> Result<(), WorkerQuitReason<StreamableHttpError<C::Error>>> {
        if pending_stream_response_ids.is_empty() {
            return Ok(());
        }

        let pending_ids = std::mem::take(pending_stream_response_ids);
        for id in pending_ids {
            context
                .send_to_handler(ServerJsonRpcMessage::error(
                    ErrorData::internal_error(
                        "streamable HTTP session was re-initialized before the response arrived",
                        None,
                    ),
                    Some(id),
                ))
                .await?;
        }
        Ok(())
    }

    /// Convert a raw SSE stream into a JSON-RPC message stream without
    /// reconnection logic.
    fn raw_sse_to_jsonrpc(
        stream: BoxedSseStream,
    ) -> impl Stream<Item = Result<ServerJsonRpcMessage, StreamableHttpError<C::Error>>> + Send + 'static
    {
        stream.filter_map(|event| async {
            match event {
                Err(e) => Some(Err(StreamableHttpError::Sse(e))),
                Ok(sse) => {
                    let is_message =
                        matches!(sse.event.as_deref(), None | Some("") | Some("message"));
                    if !is_message {
                        return None;
                    }
                    let data = sse.data?;
                    if data.trim().is_empty() {
                        return None;
                    }
                    match serde_json::from_str::<ServerJsonRpcMessage>(&data) {
                        Ok(msg) => Some(Ok(msg)),
                        Err(e) => {
                            tracing::debug!("failed to deserialize server message: {e}");
                            None
                        }
                    }
                }
            }
        })
    }

    /// Convert an SSE stream into JSON-RPC messages with reconnect semantics.
    ///
    /// This is used for request-scoped SSE responses as well as the standalone
    /// GET stream. A request-scoped stream can close before its response arrives,
    /// and SEP-1699 requires the client to honor `retry` and resume with
    /// `Last-Event-ID` in that case.
    fn reconnecting_sse_to_jsonrpc(
        stream: BoxedSseStream,
        client: C,
        session_id: Arc<str>,
        uri: Arc<str>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
        retry_config: Arc<dyn SseRetryPolicy>,
    ) -> impl Stream<Item = Result<ServerJsonRpcMessage, StreamableHttpError<C::Error>>> + Send + 'static
    {
        SseAutoReconnectStream::new(
            stream,
            StreamableHttpClientReconnect {
                client,
                session_id,
                uri,
                auth_header,
                custom_headers,
            },
            retry_config,
        )
    }

    /// Convert a POST response SSE stream into JSON-RPC messages.
    ///
    /// Stateful sessions can resume via GET when the response stream closes
    /// before the server sends the matching JSON-RPC response. Stateless
    /// transports do not have enough state to resume, so they keep the raw
    /// SSE-to-JSON-RPC mapping.
    fn response_sse_to_jsonrpc(
        stream: BoxedSseStream,
        session_id: Option<Arc<str>>,
        client: C,
        uri: Arc<str>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
        retry_config: Arc<dyn SseRetryPolicy>,
    ) -> BoxStream<'static, Result<ServerJsonRpcMessage, StreamableHttpError<C::Error>>> {
        match session_id {
            Some(session_id) => Self::reconnecting_sse_to_jsonrpc(
                stream,
                client,
                session_id,
                uri,
                auth_header,
                custom_headers,
                retry_config,
            )
            .boxed(),
            None => Self::raw_sse_to_jsonrpc(stream).boxed(),
        }
    }

    async fn execute_sse_stream(
        sse_stream: impl Stream<Item = Result<ServerJsonRpcMessage, StreamableHttpError<C::Error>>>
        + Send
        + 'static,
        sse_worker_tx: tokio::sync::mpsc::Sender<ServerJsonRpcMessage>,
        close_on_response: bool,
        ct: CancellationToken,
    ) -> Result<(), StreamableHttpError<C::Error>> {
        let mut sse_stream = std::pin::pin!(sse_stream);
        loop {
            let message = tokio::select! {
                event = sse_stream.next() => {
                    event
                }
                _ = ct.cancelled() => {
                    tracing::debug!("cancelled");
                    break;
                }
            };
            let Some(message) = message.transpose()? else {
                break;
            };
            let is_response = matches!(
                message,
                ServerJsonRpcMessage::Response(_) | ServerJsonRpcMessage::Error(_)
            );
            let yield_result = sse_worker_tx.send(message).await;
            if yield_result.is_err() {
                tracing::trace!("streamable http transport worker dropped, exiting");
                break;
            }
            if close_on_response && is_response {
                tracing::debug!("got response, draining sse stream for connection reuse");
                // Consume the remaining stream so the HTTP/1.1 connection
                // returns to the pool cleanly.
                let _ = tokio::time::timeout(std::time::Duration::from_millis(50), async {
                    while sse_stream.next().await.is_some() {}
                })
                .await;
                break;
            }
        }
        Ok(())
    }

    /// Performs a transparent re-initialization handshake after a session-expired 404.
    ///
    /// Takes an owned clone of the client (avoiding `&self` across `.await` so the
    /// future remains `Send` without requiring `C: Sync`).  POSTs the saved
    /// initialize request without a session ID, extracts the new session ID and
    /// protocol version, sends `notifications/initialized`, and returns the new
    /// `(session_id, protocol_headers)` pair.  The init result message is **not**
    /// forwarded to the handler because the handler already processed the original
    /// initialization.
    async fn perform_reinitialization(
        client: C,
        saved_init_request: ClientJsonRpcMessage,
        uri: Arc<str>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<(Option<Arc<str>>, HashMap<HeaderName, HeaderValue>), StreamableHttpError<C::Error>>
    {
        let (init_msg, new_session_id_str) = client
            .post_message(
                uri.clone(),
                saved_init_request,
                None,
                auth_header.clone(),
                custom_headers.clone(),
            )
            .await?
            .expect_initialized::<C::Error>()
            .await?;

        let new_session_id: Option<Arc<str>> = new_session_id_str.map(|s| Arc::from(s.as_str()));

        let (negotiated_version, new_protocol_headers) =
            negotiate_version_headers(&init_msg, custom_headers);

        let initialized_notification = ClientJsonRpcMessage::notification(
            ClientNotification::InitializedNotification(InitializedNotification {
                method: Default::default(),
                extensions: Default::default(),
            }),
        );
        // SEP-2243: notifications carry no Mcp-Param-*, so an empty tool cache suffices.
        let initialized_headers = build_request_headers(
            &new_protocol_headers,
            &initialized_notification,
            &HashMap::new(),
            &negotiated_version,
        );
        client
            .post_message(
                uri,
                initialized_notification,
                new_session_id.clone(),
                auth_header,
                initialized_headers,
            )
            .await?
            .expect_accepted_or_json::<C::Error>()?;

        Ok((new_session_id, new_protocol_headers))
    }
}

impl<C: StreamableHttpClient> Worker for StreamableHttpClientWorker<C> {
    type Role = RoleClient;
    type Error = StreamableHttpError<C::Error>;
    fn err_closed() -> Self::Error {
        StreamableHttpError::TransportChannelClosed
    }
    fn err_join(e: tokio::task::JoinError) -> Self::Error {
        StreamableHttpError::TokioJoinError(e)
    }
    fn config(&self) -> super::worker::WorkerConfig {
        super::worker::WorkerConfig {
            name: Some("StreamableHttpClientWorker".into()),
            channel_buffer_capacity: self.config.channel_buffer_capacity,
        }
    }
    async fn run(
        self,
        mut context: super::worker::WorkerContext<Self>,
    ) -> Result<(), WorkerQuitReason<Self::Error>> {
        let channel_buffer_capacity = self.config.channel_buffer_capacity;
        let (sse_worker_tx, mut sse_worker_rx) =
            tokio::sync::mpsc::channel::<ServerJsonRpcMessage>(channel_buffer_capacity);
        let config = self.config.clone();
        let transport_task_ct = context.cancellation_token.clone();
        let _drop_guard = transport_task_ct.clone().drop_guard();
        let WorkerSendRequest {
            responder,
            message: initialize_request,
        } = context.recv_from_handler().await?;
        let saved_init_request = initialize_request.clone();
        let (message, session_id) = match self
            .client
            .post_message(
                config.uri.clone(),
                initialize_request,
                None,
                config.auth_header.clone(),
                config.custom_headers.clone(),
            )
            .await
        {
            Ok(res) => {
                let _ = responder.send(Ok(()));
                res.expect_initialized::<C::Error>().await.map_err(
                    WorkerQuitReason::fatal_context("process initialize response"),
                )?
            }
            Err(err) => {
                let msg = format!("{:?}", err);
                let _ = responder.send(Err(err));
                return Err(WorkerQuitReason::fatal(
                    StreamableHttpError::TransportChannelClosed,
                    msg,
                ));
            }
        };
        let mut session_id: Option<Arc<str>> = if let Some(session_id) = session_id {
            Some(session_id.into())
        } else {
            if !self.config.allow_stateless {
                return Err(WorkerQuitReason::fatal(
                    StreamableHttpError::<C::Error>::MissingSessionIdInResponse,
                    "process initialize response",
                ));
            }
            None
        };
        let (negotiated_version, mut protocol_headers) =
            negotiate_version_headers(&message, config.custom_headers.clone());
        // SEP-2243: tool input schemas (name -> schema) cached from tools/list responses,
        // used to promote annotated tools/call arguments to Mcp-Param-* headers.
        let mut tool_header_cache: HashMap<String, Arc<JsonObject>> = HashMap::new();

        // Store session info for cleanup when run() exits (not spawned, so cleanup completes before close() returns)
        let mut session_cleanup_info = session_id.as_ref().map(|sid| SessionCleanupInfo {
            client: self.client.clone(),
            uri: config.uri.clone(),
            session_id: sid.clone(),
            auth_header: config.auth_header.clone(),
            protocol_headers: protocol_headers.clone(),
        });

        context.send_to_handler(message).await?;
        let initialized_notification = context.recv_from_handler().await?;
        // expect a initialized response
        let initialized_headers = build_request_headers(
            &protocol_headers,
            &initialized_notification.message,
            &tool_header_cache,
            &negotiated_version,
        );
        self.client
            .post_message(
                config.uri.clone(),
                initialized_notification.message,
                session_id.clone(),
                config.auth_header.clone(),
                initialized_headers,
            )
            .await
            .map_err(WorkerQuitReason::fatal_context(
                "send initialized notification",
            ))?
            .expect_accepted_or_json::<C::Error>()
            .map_err(WorkerQuitReason::fatal_context(
                "process initialized notification response",
            ))?;
        let _ = initialized_notification.responder.send(Ok(()));
        #[allow(clippy::large_enum_variant)]
        enum Event<W: Worker, E: std::error::Error + Send + Sync + 'static> {
            ClientMessage(WorkerSendRequest<W>),
            ServerMessage(ServerJsonRpcMessage),
            StreamResult(Result<(), StreamableHttpError<E>>),
        }
        let mut streams = tokio::task::JoinSet::new();
        let mut pending_stream_response_ids = HashSet::new();
        if let Some(session_id) = &session_id {
            let client = self.client.clone();
            let uri = config.uri.clone();
            let session_id = session_id.clone();
            let auth_header = config.auth_header.clone();
            let retry_config = self.config.retry_config.clone();
            let sse_worker_tx = sse_worker_tx.clone();
            let transport_task_ct = transport_task_ct.clone();
            let config_uri = config.uri.clone();
            let config_auth_header = config.auth_header.clone();
            let spawn_headers = protocol_headers.clone();

            streams.spawn(async move {
                match client
                    .get_stream(
                        uri.clone(),
                        session_id.clone(),
                        None,
                        auth_header.clone(),
                        spawn_headers.clone(),
                    )
                    .await
                {
                    Ok(stream) => {
                        let sse_stream = SseAutoReconnectStream::new(
                            stream,
                            StreamableHttpClientReconnect {
                                client: client.clone(),
                                session_id: session_id.clone(),
                                uri: config_uri,
                                auth_header: config_auth_header,
                                custom_headers: spawn_headers,
                            },
                            retry_config,
                        );
                        Self::execute_sse_stream(
                            sse_stream,
                            sse_worker_tx,
                            false,
                            transport_task_ct.child_token(),
                        )
                        .await
                    }
                    Err(StreamableHttpError::ServerDoesNotSupportSse) => {
                        tracing::debug!("server doesn't support sse, skip common stream");
                        Ok(())
                    }
                    Err(e) => {
                        // fail to get common stream
                        tracing::error!("fail to get common stream: {e}");
                        Err(e)
                    }
                }
            });
        }
        // Main event loop - capture exit reason so we can do cleanup before returning
        let loop_result: Result<(), WorkerQuitReason<Self::Error>> = 'main_loop: loop {
            let event = tokio::select! {
                _ = transport_task_ct.cancelled() => {
                    tracing::debug!("cancelled");
                    break 'main_loop Err(WorkerQuitReason::Cancelled);
                }
                message = context.recv_from_handler() => {
                    match message {
                        Ok(msg) => Event::ClientMessage(msg),
                        Err(e) => break 'main_loop Err(e),
                    }
                },
                message = sse_worker_rx.recv() => {
                    let Some(message) = message else {
                        tracing::trace!("transport dropped, exiting");
                        break 'main_loop Err(WorkerQuitReason::HandlerTerminated);
                    };
                    Event::ServerMessage(message)
                },
                terminated_stream = streams.join_next(), if !streams.is_empty() => {
                    match terminated_stream {
                        Some(result) => {
                            Event::StreamResult(result.map_err(StreamableHttpError::TokioJoinError).and_then(std::convert::identity))
                        }
                        None => {
                            continue
                        }
                    }
                }
            };
            match event {
                Event::ClientMessage(send_request) => {
                    let WorkerSendRequest { message, responder } = send_request;
                    let request_id = Self::client_request_id(&message);
                    // Pass a clone to the first attempt so `message` is retained for a
                    // potential re-init retry. `post_message` takes ownership and the
                    // trait cannot be changed, so the clone is unavoidable.
                    let request_headers = build_request_headers(
                        &protocol_headers,
                        &message,
                        &tool_header_cache,
                        &negotiated_version,
                    );
                    let response = self
                        .client
                        .post_message(
                            config.uri.clone(),
                            message.clone(),
                            session_id.clone(),
                            config.auth_header.clone(),
                            request_headers,
                        )
                        .await;
                    let send_result = match response {
                        Err(StreamableHttpError::SessionExpired) => {
                            if !config.reinit_on_expired_session {
                                Err(StreamableHttpError::SessionExpired)
                            } else {
                                // The server discarded the session (HTTP 404). Perform a
                                // fresh handshake once and replay the original message.
                                tracing::info!(
                                    "session expired (HTTP 404), attempting transparent re-initialization"
                                );
                                match Self::perform_reinitialization(
                                    self.client.clone(),
                                    saved_init_request.clone(),
                                    config.uri.clone(),
                                    config.auth_header.clone(),
                                    config.custom_headers.clone(),
                                )
                                .await
                                {
                                    Ok((new_session_id, new_protocol_headers)) => {
                                        // Old streams hold the stale session ID. Stop them first
                                        // so no late stale-session messages can arrive after the
                                        // pending requests below are completed.
                                        streams.abort_all();
                                        while streams.join_next().await.is_some() {}

                                        // Forward any already queued response messages and fail
                                        // the remaining accepted requests so callers do not wait
                                        // forever for responses that can no longer arrive.
                                        Self::drain_queued_stream_messages(
                                            &mut sse_worker_rx,
                                            &mut context,
                                            &mut pending_stream_response_ids,
                                        )
                                        .await?;
                                        Self::fail_pending_stream_responses(
                                            &mut context,
                                            &mut pending_stream_response_ids,
                                        )
                                        .await?;

                                        session_id = new_session_id;
                                        protocol_headers = new_protocol_headers;
                                        session_cleanup_info =
                                            session_id.as_ref().map(|sid| SessionCleanupInfo {
                                                client: self.client.clone(),
                                                uri: config.uri.clone(),
                                                session_id: sid.clone(),
                                                auth_header: config.auth_header.clone(),
                                                protocol_headers: protocol_headers.clone(),
                                            });

                                        if let Some(new_sid) = &session_id {
                                            let client = self.client.clone();
                                            let uri = config.uri.clone();
                                            let new_sid = new_sid.clone();
                                            let auth_header = config.auth_header.clone();
                                            let retry_config = self.config.retry_config.clone();
                                            let sse_tx = sse_worker_tx.clone();
                                            let task_ct = transport_task_ct.clone();
                                            let config_uri = config.uri.clone();
                                            let config_auth = config.auth_header.clone();
                                            let spawn_headers = protocol_headers.clone();
                                            streams.spawn(async move {
                                            match client
                                                .get_stream(
                                                    uri,
                                                    new_sid.clone(),
                                                    None,
                                                    auth_header.clone(),
                                                    spawn_headers.clone(),
                                                )
                                                .await
                                            {
                                                Ok(stream) => {
                                                    let sse_stream = SseAutoReconnectStream::new(
                                                        stream,
                                                        StreamableHttpClientReconnect {
                                                            client: client.clone(),
                                                            session_id: new_sid,
                                                            uri: config_uri,
                                                            auth_header: config_auth,
                                                            custom_headers: spawn_headers,
                                                        },
                                                        retry_config,
                                                    );
                                                    Self::execute_sse_stream(
                                                        sse_stream,
                                                        sse_tx,
                                                        false,
                                                        task_ct.child_token(),
                                                    )
                                                    .await
                                                }
                                                Err(StreamableHttpError::ServerDoesNotSupportSse) => {
                                                    tracing::debug!(
                                                        "server doesn't support sse after re-init"
                                                    );
                                                    Ok(())
                                                }
                                                Err(e) => {
                                                    tracing::error!(
                                                        "fail to get common stream after re-init: {e}"
                                                    );
                                                    Err(e)
                                                }
                                            }
                                        });
                                        }

                                        let retry_headers = build_request_headers(
                                            &protocol_headers,
                                            &message,
                                            &tool_header_cache,
                                            &negotiated_version,
                                        );
                                        let retry_response = self
                                            .client
                                            .post_message(
                                                config.uri.clone(),
                                                message,
                                                session_id.clone(),
                                                config.auth_header.clone(),
                                                retry_headers,
                                            )
                                            .await;
                                        match retry_response {
                                            Err(e) => Err(e),
                                            Ok(StreamableHttpPostResponse::Accepted) => {
                                                Self::mark_stream_response_pending(
                                                    &mut pending_stream_response_ids,
                                                    request_id,
                                                );
                                                tracing::trace!(
                                                    "client message accepted after re-init"
                                                );
                                                Ok(())
                                            }
                                            Ok(StreamableHttpPostResponse::Json(msg, ..)) => {
                                                cache_tools_from_response(
                                                    &mut tool_header_cache,
                                                    &msg,
                                                );
                                                context.send_to_handler(msg).await?;
                                                Ok(())
                                            }
                                            Ok(StreamableHttpPostResponse::Sse(stream, ..)) => {
                                                Self::mark_stream_response_pending(
                                                    &mut pending_stream_response_ids,
                                                    request_id,
                                                );
                                                let sse_stream = Self::response_sse_to_jsonrpc(
                                                    stream,
                                                    session_id.clone(),
                                                    self.client.clone(),
                                                    config.uri.clone(),
                                                    config.auth_header.clone(),
                                                    protocol_headers.clone(),
                                                    self.config.retry_config.clone(),
                                                );
                                                streams.spawn(Self::execute_sse_stream(
                                                    sse_stream,
                                                    sse_worker_tx.clone(),
                                                    true,
                                                    transport_task_ct.child_token(),
                                                ));
                                                tracing::trace!("got new sse stream after re-init");
                                                Ok(())
                                            }
                                        }
                                    }
                                    Err(reinit_err) => Err(reinit_err),
                                }
                            } // else enable_reinit_on_expired_session
                        }
                        Err(e) => Err(e),
                        Ok(StreamableHttpPostResponse::Accepted) => {
                            Self::mark_stream_response_pending(
                                &mut pending_stream_response_ids,
                                request_id,
                            );
                            tracing::trace!("client message accepted");
                            Ok(())
                        }
                        Ok(StreamableHttpPostResponse::Json(message, ..)) => {
                            cache_tools_from_response(&mut tool_header_cache, &message);
                            context.send_to_handler(message).await?;
                            Ok(())
                        }
                        Ok(StreamableHttpPostResponse::Sse(stream, ..)) => {
                            Self::mark_stream_response_pending(
                                &mut pending_stream_response_ids,
                                request_id,
                            );
                            let sse_stream = Self::response_sse_to_jsonrpc(
                                stream,
                                session_id.clone(),
                                self.client.clone(),
                                config.uri.clone(),
                                config.auth_header.clone(),
                                protocol_headers.clone(),
                                self.config.retry_config.clone(),
                            );
                            streams.spawn(Self::execute_sse_stream(
                                sse_stream,
                                sse_worker_tx.clone(),
                                true,
                                transport_task_ct.child_token(),
                            ));
                            tracing::trace!("got new sse stream");
                            Ok(())
                        }
                    };
                    let _ = responder.send(send_result);
                }
                Event::ServerMessage(json_rpc_message) => {
                    Self::clear_stream_response_pending(
                        &mut pending_stream_response_ids,
                        &json_rpc_message,
                    );
                    cache_tools_from_response(&mut tool_header_cache, &json_rpc_message);
                    // send the message to the handler
                    if let Err(e) = context.send_to_handler(json_rpc_message).await {
                        break 'main_loop Err(e);
                    }
                }
                Event::StreamResult(result) => {
                    if result.is_err() {
                        tracing::warn!(
                            "sse client event stream terminated with error: {:?}",
                            result
                        );
                    }
                }
            }
        };

        // Cleanup session before returning (ensures close() waits for session deletion)
        // Use a timeout to prevent indefinite hangs if the server is unresponsive
        if let Some(cleanup) = session_cleanup_info {
            const SESSION_CLEANUP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
            let cleanup_session_id = cleanup.session_id.clone();
            match tokio::time::timeout(
                SESSION_CLEANUP_TIMEOUT,
                cleanup.client.delete_session(
                    cleanup.uri,
                    cleanup.session_id,
                    cleanup.auth_header,
                    cleanup.protocol_headers,
                ),
            )
            .await
            {
                Ok(Ok(_)) => {
                    tracing::info!(
                        session_id = cleanup_session_id.as_ref(),
                        "delete session success"
                    )
                }
                Ok(Err(StreamableHttpError::ServerDoesNotSupportDeleteSession)) => {
                    tracing::info!(
                        session_id = cleanup_session_id.as_ref(),
                        "server doesn't support delete session"
                    )
                }
                Ok(Err(e)) => {
                    tracing::error!(
                        session_id = cleanup_session_id.as_ref(),
                        "fail to delete session: {e}"
                    );
                }
                Err(_elapsed) => {
                    tracing::warn!(
                        session_id = cleanup_session_id.as_ref(),
                        "session cleanup timed out after {:?}",
                        SESSION_CLEANUP_TIMEOUT
                    );
                }
            }
        }

        loop_result
    }
}

/// A client-agnostic HTTP transport for RMCP that supports streaming responses.
///
/// This transport allows you to choose your preferred HTTP client implementation
/// by implementing the [`StreamableHttpClient`] trait. The transport handles
/// session management, SSE streaming, and automatic reconnection.
///
/// # Usage
///
/// ## Using reqwest
///
/// ```rust,no_run
/// use rmcp::transport::StreamableHttpClientTransport;
///
/// // Enable the reqwest feature in Cargo.toml:
/// // rmcp = { version = "0.5", features = ["transport-streamable-http-client-reqwest"] }
///
/// let transport = StreamableHttpClientTransport::from_uri("http://localhost:8000/mcp");
/// ```
///
/// ## Using a custom HTTP client
///
/// ```rust,no_run
/// use rmcp::transport::streamable_http_client::{
///     StreamableHttpClient,
///     StreamableHttpClientTransport,
///     StreamableHttpClientTransportConfig
/// };
/// use std::sync::Arc;
/// use std::collections::HashMap;
/// use futures::stream::BoxStream;
/// use rmcp::model::ClientJsonRpcMessage;
/// use http::{HeaderName, HeaderValue};
/// use sse_stream::{Sse, Error as SseError};
///
/// #[derive(Clone)]
/// struct MyHttpClient;
///
/// #[derive(Debug, thiserror::Error)]
/// struct MyError;
///
/// impl std::fmt::Display for MyError {
///     fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
///         write!(f, "MyError")
///     }
/// }
///
/// impl StreamableHttpClient for MyHttpClient {
///     type Error = MyError;
///
///     async fn post_message(
///         &self,
///         _uri: Arc<str>,
///         _message: ClientJsonRpcMessage,
///         _session_id: Option<Arc<str>>,
///         _auth_header: Option<String>,
///         _custom_headers: HashMap<HeaderName, HeaderValue>,
///     ) -> Result<rmcp::transport::streamable_http_client::StreamableHttpPostResponse, rmcp::transport::streamable_http_client::StreamableHttpError<Self::Error>> {
///         todo!()
///     }
///
///     async fn delete_session(
///         &self,
///         _uri: Arc<str>,
///         _session_id: Arc<str>,
///         _auth_header: Option<String>,
///         _custom_headers: HashMap<HeaderName, HeaderValue>,
///     ) -> Result<(), rmcp::transport::streamable_http_client::StreamableHttpError<Self::Error>> {
///         todo!()
///     }
///
///     async fn get_stream(
///         &self,
///         _uri: Arc<str>,
///         _session_id: Arc<str>,
///         _last_event_id: Option<String>,
///         _auth_header: Option<String>,
///         _custom_headers: HashMap<HeaderName, HeaderValue>,
///     ) -> Result<BoxStream<'static, Result<Sse, SseError>>, rmcp::transport::streamable_http_client::StreamableHttpError<Self::Error>> {
///         todo!()
///     }
/// }
///
/// let transport = StreamableHttpClientTransport::with_client(
///     MyHttpClient,
///     StreamableHttpClientTransportConfig::with_uri("http://localhost:8000/mcp")
/// );
/// ```
///
/// # Feature Flags
///
/// - `transport-streamable-http-client`: Base feature providing the generic transport infrastructure
/// - `transport-streamable-http-client-reqwest`: Includes reqwest HTTP client support with convenience methods
pub type StreamableHttpClientTransport<C> = WorkerTransport<StreamableHttpClientWorker<C>>;

impl<C: StreamableHttpClient> StreamableHttpClientTransport<C> {
    /// Creates a new transport with a custom HTTP client implementation.
    ///
    /// This method allows you to use any HTTP client that implements the [`StreamableHttpClient`] trait.
    /// Use this when you want to use a custom HTTP client or when the reqwest feature is not enabled.
    ///
    /// # Arguments
    ///
    /// * `client` - Your HTTP client implementation
    /// * `config` - Transport configuration including the server URI
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use rmcp::transport::streamable_http_client::{
    ///     StreamableHttpClient,
    ///     StreamableHttpClientTransport,
    ///     StreamableHttpClientTransportConfig
    /// };
    /// use std::sync::Arc;
    /// use std::collections::HashMap;
    /// use futures::stream::BoxStream;
    /// use rmcp::model::ClientJsonRpcMessage;
    /// use http::{HeaderName, HeaderValue};
    /// use sse_stream::{Sse, Error as SseError};
    ///
    /// // Define your custom client
    /// #[derive(Clone)]
    /// struct MyHttpClient;
    ///
    /// #[derive(Debug, thiserror::Error)]
    /// struct MyError;
    ///
    /// impl std::fmt::Display for MyError {
    ///     fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    ///         write!(f, "MyError")
    ///     }
    /// }
    ///
    /// impl StreamableHttpClient for MyHttpClient {
    ///     type Error = MyError;
    ///
    ///     async fn post_message(
    ///         &self,
    ///         _uri: Arc<str>,
    ///         _message: ClientJsonRpcMessage,
    ///         _session_id: Option<Arc<str>>,
    ///         _auth_header: Option<String>,
    ///         _custom_headers: HashMap<HeaderName, HeaderValue>,
    ///     ) -> Result<rmcp::transport::streamable_http_client::StreamableHttpPostResponse, rmcp::transport::streamable_http_client::StreamableHttpError<Self::Error>> {
    ///         todo!()
    ///     }
    ///
    ///     async fn delete_session(
    ///         &self,
    ///         _uri: Arc<str>,
    ///         _session_id: Arc<str>,
    ///         _auth_header: Option<String>,
    ///         _custom_headers: HashMap<HeaderName, HeaderValue>,
    ///     ) -> Result<(), rmcp::transport::streamable_http_client::StreamableHttpError<Self::Error>> {
    ///         todo!()
    ///     }
    ///
    ///     async fn get_stream(
    ///         &self,
    ///         _uri: Arc<str>,
    ///         _session_id: Arc<str>,
    ///         _last_event_id: Option<String>,
    ///         _auth_header: Option<String>,
    ///         _custom_headers: HashMap<HeaderName, HeaderValue>,
    ///     ) -> Result<BoxStream<'static, Result<Sse, SseError>>, rmcp::transport::streamable_http_client::StreamableHttpError<Self::Error>> {
    ///         todo!()
    ///     }
    /// }
    ///
    /// let transport = StreamableHttpClientTransport::with_client(
    ///     MyHttpClient,
    ///     StreamableHttpClientTransportConfig::with_uri("http://localhost:8000/mcp")
    /// );
    /// ```
    pub fn with_client(client: C, config: StreamableHttpClientTransportConfig) -> Self {
        let worker = StreamableHttpClientWorker::new(client, config);
        WorkerTransport::spawn(worker)
    }
}
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct StreamableHttpClientTransportConfig {
    pub uri: Arc<str>,
    pub retry_config: Arc<dyn SseRetryPolicy>,
    pub channel_buffer_capacity: usize,
    /// if true, the transport will not require a session to be established
    pub allow_stateless: bool,
    /// The value to send in the authorization header
    pub auth_header: Option<String>,
    /// Custom HTTP headers to include with every request
    pub custom_headers: HashMap<HeaderName, HeaderValue>,
    /// Enables transparent recovery when the server reports an expired session (`HTTP 404`).
    ///
    /// When enabled, the transport performs one automatic recovery attempt:
    /// 1. Replays the original `initialize` handshake to create a new session.
    /// 2. Re-establishes streaming state for that session.
    /// 3. Retries the in-flight request that failed with `SessionExpired`.
    ///
    /// This recovery is best-effort and bounded to a single attempt. If recovery fails,
    /// the original failure path is preserved and the error is returned to the caller.
    pub reinit_on_expired_session: bool,
}

impl StreamableHttpClientTransportConfig {
    pub fn with_uri(uri: impl Into<Arc<str>>) -> Self {
        Self {
            uri: uri.into(),
            ..Default::default()
        }
    }

    /// Set the authorization header to send with requests
    ///
    /// # Arguments
    ///
    /// * `value` - A bearer token without the `Bearer ` prefix
    pub fn auth_header<T: Into<String>>(mut self, value: T) -> Self {
        // set our authorization header
        self.auth_header = Some(value.into());
        self
    }

    /// Set custom HTTP headers to include with every request
    ///
    /// # Arguments
    ///
    /// * `custom_headers` - A HashMap of header names to header values
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use std::collections::HashMap;
    /// use http::{HeaderName, HeaderValue};
    /// use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
    ///
    /// let mut headers = HashMap::new();
    /// headers.insert(
    ///     HeaderName::from_static("x-custom-header"),
    ///     HeaderValue::from_static("custom-value")
    /// );
    ///
    /// let config = StreamableHttpClientTransportConfig::with_uri("http://localhost:8000")
    ///     .custom_headers(headers);
    /// ```
    pub fn custom_headers(mut self, custom_headers: HashMap<HeaderName, HeaderValue>) -> Self {
        self.custom_headers = custom_headers;
        self
    }

    /// Set whether the transport should attempt transparent re-initialization on session expiration
    /// See [`Self::reinit_on_expired_session`] for details.
    /// # Example
    /// ```rust,no_run
    /// use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
    /// let config = StreamableHttpClientTransportConfig::with_uri("http://localhost:8000")
    ///     .reinit_on_expired_session(true);
    /// ```
    pub fn reinit_on_expired_session(mut self, enable: bool) -> Self {
        self.reinit_on_expired_session = enable;
        self
    }
}

impl Default for StreamableHttpClientTransportConfig {
    fn default() -> Self {
        Self {
            uri: "localhost".into(),
            retry_config: Arc::new(ExponentialBackoff::default()),
            channel_buffer_capacity: 16,
            allow_stateless: true,
            auth_header: None,
            custom_headers: HashMap::new(),
            reinit_on_expired_session: true,
        }
    }
}
