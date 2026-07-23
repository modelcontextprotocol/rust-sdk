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

use super::common::client_side_sse::{
    DEFAULT_MAX_SSE_EVENT_SIZE, ExponentialBackoff, SseRetryPolicy, SseStreamReconnect,
};
use crate::{
    RoleClient,
    model::{
        ClientJsonRpcMessage, ClientNotification, ClientRequest, ErrorData, GetMeta,
        InitializedNotification, JsonObject, ProtocolVersion, RequestId, ServerJsonRpcMessage,
        ServerResult,
    },
    transport::{
        common::{client_side_sse::SseAutoReconnectStream, mcp_headers},
        worker::{Worker, WorkerQuitReason, WorkerSendRequest, WorkerTransport},
    },
};

type BoxedSseStream = BoxStream<'static, Result<Sse, SseError>>;
type SseTaskResult<E> = (Option<RequestId>, Result<(), StreamableHttpError<E>>);
const SESSION_CLEANUP_TIMEOUT: Duration = Duration::from_secs(5);

fn build_request_headers(
    base: &HashMap<HeaderName, HeaderValue>,
    message: &ClientJsonRpcMessage,
    tool_cache: &HashMap<String, Arc<JsonObject>>,
    version: &ProtocolVersion,
) -> HashMap<HeaderName, HeaderValue> {
    use serde_json::Value;

    let mut headers = base.clone();
    if *version >= ProtocolVersion::STANDARD_HEADERS
        && let Ok(value) = serde_json::to_value(message)
    {
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
    headers
}

fn request_version_headers(
    base: &HashMap<HeaderName, HeaderValue>,
    message: &ClientJsonRpcMessage,
    fallback: &ProtocolVersion,
    tool_cache: &HashMap<String, Arc<JsonObject>>,
) -> (ProtocolVersion, HashMap<HeaderName, HeaderValue>) {
    let version = match message {
        ClientJsonRpcMessage::Request(request) => request
            .request
            .get_meta()
            .protocol_version()
            .unwrap_or_else(|| fallback.clone()),
        _ => fallback.clone(),
    };
    let mut headers = build_request_headers(base, message, tool_cache, &version);
    if let Ok(value) = HeaderValue::from_str(version.as_str()) {
        headers.insert(HeaderName::from_static("mcp-protocol-version"), value);
    }
    (version, headers)
}

fn cache_tools_from_response(
    cache: &mut HashMap<String, Arc<JsonObject>>,
    message: &mut ServerJsonRpcMessage,
    protocol_version: &ProtocolVersion,
) {
    if protocol_version < &ProtocolVersion::STANDARD_HEADERS {
        return;
    }
    if let ServerJsonRpcMessage::Response(response) = message
        && let ServerResult::ListToolsResult(list) = &mut response.result
    {
        list.tools.retain(|tool| {
                let Err(reason) =
                    mcp_headers::validate_param_header_annotations(&tool.input_schema)
                else {
                    cache.insert(tool.name.to_string(), tool.input_schema.clone());
                    return true;
                };
                tracing::warn!(tool = %tool.name, "rejecting invalid x-mcp-header annotations: {reason}");
                false
            });
    }
}

fn negotiate_version_headers(
    init_response: &ServerJsonRpcMessage,
    base: HashMap<HeaderName, HeaderValue>,
) -> (ProtocolVersion, HashMap<HeaderName, HeaderValue>) {
    let mut version = ProtocolVersion::default();
    let mut headers = base;
    if let ServerJsonRpcMessage::Response(response) = init_response
        && let ServerResult::InitializeResult(init_result) = &response.result
    {
        version = init_result.protocol_version.clone();
        // HeaderName::from_static requires lowercase
        if let Ok(hv) = HeaderValue::from_str(init_result.protocol_version.as_str()) {
            headers.insert(HeaderName::from_static("mcp-protocol-version"), hv);
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

#[expect(
    clippy::large_enum_variant,
    reason = "boxing the streaming response would add an allocation to the common response path"
)]
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

/// HTTP backend used by [`StreamableHttpClientTransport`].
///
/// Custom implementations that parse SSE responses must override
/// [`Self::post_message_with_max_sse_event_size`] and
/// [`Self::get_stream_with_max_sse_event_size`] to enforce the transport's
/// configured event-size limit.
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
    /// Send a message while enforcing a maximum raw SSE event size.
    ///
    /// `max_sse_event_size` is not a per-request option: it is the
    /// transport-wide [`StreamableHttpClientTransportConfig::max_sse_event_size`]
    /// value, passed identically on every call because the limit must be applied
    /// inside the client (at the raw byte layer, before SSE parsing) rather than
    /// by the caller.
    ///
    /// Custom clients that parse SSE responses should override this method.
    /// The default implementation delegates to [`Self::post_message`].
    fn post_message_with_max_sse_event_size(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
        _max_sse_event_size: usize,
    ) -> impl Future<Output = Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>>>
    + Send
    + '_ {
        self.post_message(uri, message, session_id, auth_header, custom_headers)
    }
    fn delete_session(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> impl Future<Output = Result<(), StreamableHttpError<Self::Error>>> + Send + '_;
    /// Open an SSE stream, optionally scoped to a legacy session.
    ///
    /// `session_id` is `None` when resuming a stateless response using only
    /// `last_event_id`.
    fn get_stream(
        &self,
        uri: Arc<str>,
        session_id: Option<Arc<str>>,
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
    /// Open an SSE stream while enforcing a maximum raw event size.
    ///
    /// `max_sse_event_size` is not a per-request option: it is the
    /// transport-wide [`StreamableHttpClientTransportConfig::max_sse_event_size`]
    /// value, passed identically on every call because the limit must be applied
    /// inside the client (at the raw byte layer, before SSE parsing) rather than
    /// by the caller.
    ///
    /// Custom clients that parse SSE responses should override this method.
    /// The default implementation delegates to [`Self::get_stream`].
    fn get_stream_with_max_sse_event_size(
        &self,
        uri: Arc<str>,
        session_id: Option<Arc<str>>,
        last_event_id: Option<String>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
        _max_sse_event_size: usize,
    ) -> impl Future<
        Output = Result<
            BoxStream<'static, Result<Sse, SseError>>,
            StreamableHttpError<Self::Error>,
        >,
    > + Send
    + '_ {
        self.get_stream(uri, session_id, last_event_id, auth_header, custom_headers)
    }
}

#[non_exhaustive]
pub struct RetryConfig {
    pub max_times: Option<usize>,
    pub min_duration: Duration,
}

struct StreamableHttpClientReconnect<C> {
    pub client: C,
    pub session_id: Option<Arc<str>>,
    pub uri: Arc<str>,
    pub auth_header: Option<String>,
    pub custom_headers: HashMap<HeaderName, HeaderValue>,
    pub max_sse_event_size: usize,
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
        let max_sse_event_size = self.max_sse_event_size;
        let last_event_id = last_event_id.map(|s| s.to_owned());
        Box::pin(async move {
            client
                .get_stream_with_max_sse_event_size(
                    uri,
                    session_id,
                    last_event_id,
                    auth_header,
                    custom_headers,
                    max_sse_event_size,
                )
                .await
        })
    }

    fn map_fatal_stream_error(&mut self, error: SseError) -> Option<Self::Error> {
        Some(StreamableHttpError::Sse(error))
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
        let Some(response_id) = Self::server_response_id(message) else {
            return;
        };
        if pending_stream_response_ids.remove(response_id) {
            return;
        }
        if let Some(id) = response_id.numeric_string_value() {
            pending_stream_response_ids.remove(&RequestId::Number(id));
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

    /// Convert an SSE stream into JSON-RPC messages with reconnect semantics.
    ///
    /// This is used for request-scoped SSE responses as well as the standalone
    /// GET stream. A request-scoped stream can close before its response arrives,
    /// and SEP-1699 requires the client to honor `retry` and resume with
    /// `Last-Event-ID` in that case.
    fn reconnecting_sse_to_jsonrpc(
        stream: BoxedSseStream,
        client: C,
        session_id: Option<Arc<str>>,
        uri: Arc<str>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
        max_sse_event_size: usize,
        retry_config: Arc<dyn SseRetryPolicy>,
    ) -> impl Stream<Item = Result<ServerJsonRpcMessage, StreamableHttpError<C::Error>>> + Send + 'static
    {
        SseAutoReconnectStream::new_after_event_id(
            stream,
            StreamableHttpClientReconnect {
                client,
                session_id,
                uri,
                auth_header,
                custom_headers,
                max_sse_event_size,
            },
            retry_config,
        )
    }

    /// Convert a POST response SSE stream into JSON-RPC messages.
    ///
    /// Request-scoped streams resume via GET once the server has supplied an
    /// event ID. The session header remains optional for stateless transports.
    fn response_sse_to_jsonrpc(
        stream: BoxedSseStream,
        session_id: Option<Arc<str>>,
        client: C,
        uri: Arc<str>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
        max_sse_event_size: usize,
        retry_config: Arc<dyn SseRetryPolicy>,
    ) -> BoxStream<'static, Result<ServerJsonRpcMessage, StreamableHttpError<C::Error>>> {
        Self::reconnecting_sse_to_jsonrpc(
            stream,
            client,
            session_id,
            uri,
            auth_header,
            custom_headers,
            max_sse_event_size,
            retry_config,
        )
        .boxed()
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

    fn spawn_common_stream(
        streams: &mut tokio::task::JoinSet<SseTaskResult<C::Error>>,
        client: C,
        session_id: Arc<str>,
        config: &StreamableHttpClientTransportConfig,
        protocol_headers: HashMap<HeaderName, HeaderValue>,
        sse_worker_tx: tokio::sync::mpsc::Sender<ServerJsonRpcMessage>,
        transport_task_ct: CancellationToken,
    ) {
        let uri = config.uri.clone();
        let auth_header = config.auth_header.clone();
        let retry_config = config.retry_config.clone();
        let reconnect_uri = config.uri.clone();
        let reconnect_auth_header = config.auth_header.clone();
        let max_sse_event_size = config.max_sse_event_size;

        streams.spawn(async move {
            let result = match client
                .get_stream_with_max_sse_event_size(
                    uri,
                    Some(session_id.clone()),
                    None,
                    auth_header,
                    protocol_headers.clone(),
                    max_sse_event_size,
                )
                .await
            {
                Ok(stream) => {
                    let sse_stream = SseAutoReconnectStream::new(
                        stream,
                        StreamableHttpClientReconnect {
                            client,
                            session_id: Some(session_id),
                            uri: reconnect_uri,
                            auth_header: reconnect_auth_header,
                            custom_headers: protocol_headers,
                            max_sse_event_size,
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
                Err(error) => {
                    tracing::error!("fail to get common stream: {error}");
                    Err(error)
                }
            };
            (None, result)
        });
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
        max_sse_event_size: usize,
    ) -> Result<
        (
            Option<Arc<str>>,
            ProtocolVersion,
            HashMap<HeaderName, HeaderValue>,
        ),
        StreamableHttpError<C::Error>,
    > {
        let (init_msg, new_session_id_str) = client
            .post_message_with_max_sse_event_size(
                uri.clone(),
                saved_init_request,
                None,
                auth_header.clone(),
                custom_headers.clone(),
                max_sse_event_size,
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
            .post_message_with_max_sse_event_size(
                uri,
                initialized_notification,
                new_session_id.clone(),
                auth_header,
                initialized_headers,
                max_sse_event_size,
            )
            .await?
            .expect_accepted_or_json::<C::Error>()?;

        Ok((new_session_id, negotiated_version, new_protocol_headers))
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
            message: startup_request,
        } = context.recv_from_handler().await?;
        let is_legacy_startup = matches!(
            &startup_request,
            ClientJsonRpcMessage::Request(request)
                if matches!(&request.request, ClientRequest::InitializeRequest(_))
        );
        let mut saved_init_request = is_legacy_startup.then(|| startup_request.clone());
        let empty_tool_cache = HashMap::new();
        let (bootstrap_version, bootstrap_headers) = if is_legacy_startup {
            (ProtocolVersion::default(), config.custom_headers.clone())
        } else {
            request_version_headers(
                &config.custom_headers,
                &startup_request,
                &ProtocolVersion::default(),
                &empty_tool_cache,
            )
        };
        let (message, session_id) = match self
            .client
            .post_message_with_max_sse_event_size(
                config.uri.clone(),
                startup_request,
                None,
                config.auth_header.clone(),
                bootstrap_headers.clone(),
                config.max_sse_event_size,
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
        let mut uses_modern_http = !is_legacy_startup;
        let mut session_id: Option<Arc<str>> = if uses_modern_http {
            None
        } else if let Some(session_id) = session_id {
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
        let (mut negotiated_version, mut protocol_headers) = if is_legacy_startup {
            negotiate_version_headers(&message, config.custom_headers.clone())
        } else {
            (bootstrap_version, bootstrap_headers)
        };
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
        if is_legacy_startup {
            let initialized_notification = context.recv_from_handler().await?;
            let initialized_headers = build_request_headers(
                &protocol_headers,
                &initialized_notification.message,
                &tool_header_cache,
                &negotiated_version,
            );
            self.client
                .post_message_with_max_sse_event_size(
                    config.uri.clone(),
                    initialized_notification.message,
                    session_id.clone(),
                    config.auth_header.clone(),
                    initialized_headers,
                    config.max_sse_event_size,
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
        }
        #[expect(
            clippy::large_enum_variant,
            reason = "the event is short-lived and boxing would add allocation in the event loop"
        )]
        enum Event<W: Worker, E: std::error::Error + Send + Sync + 'static> {
            ClientMessage(WorkerSendRequest<W>),
            ServerMessage(ServerJsonRpcMessage),
            StreamResult {
                request_id: Option<RequestId>,
                result: Result<(), StreamableHttpError<E>>,
            },
        }
        let mut streams = tokio::task::JoinSet::new();
        let mut pending_stream_response_ids = HashSet::new();
        let mut request_stream_cancellations = HashMap::<RequestId, CancellationToken>::new();
        let mut awaiting_fallback_initialized = false;
        if let Some(session_id) = &session_id {
            Self::spawn_common_stream(
                &mut streams,
                self.client.clone(),
                session_id.clone(),
                &config,
                protocol_headers.clone(),
                sse_worker_tx.clone(),
                transport_task_ct.clone(),
            );
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
                            match result {
                                Ok((request_id, result)) => {
                                    Event::StreamResult { request_id, result }
                                }
                                Err(error) => Event::StreamResult {
                                    request_id: None,
                                    result: Err(StreamableHttpError::TokioJoinError(error)),
                                },
                            }
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
                    let cancellation_request_id = match &message {
                        ClientJsonRpcMessage::Notification(notification) => {
                            match &notification.notification {
                                ClientNotification::CancelledNotification(cancelled) => {
                                    cancelled.params.request_id.clone()
                                }
                                _ => None,
                            }
                        }
                        _ => None,
                    };
                    if uses_modern_http && let Some(request_id) = cancellation_request_id {
                        if let Some(stream_ct) = request_stream_cancellations.remove(&request_id) {
                            stream_ct.cancel();
                        }
                        pending_stream_response_ids.remove(&request_id);
                        let _ = responder.send(Ok(()));
                        continue;
                    }
                    let is_fallback_initialize = saved_init_request.is_none()
                        && matches!(
                            &message,
                            ClientJsonRpcMessage::Request(request)
                                if matches!(
                                    &request.request,
                                    ClientRequest::InitializeRequest(_)
                                )
                        );
                    if is_fallback_initialize {
                        saved_init_request = Some(message.clone());
                        // Servers do not assign sessions to `server/discover`, so a
                        // fallback initialize starts from a clean slate: no session
                        // ID, no cleanup state, and no streams to tear down.
                        debug_assert!(
                            session_id.is_none()
                                && session_cleanup_info.is_none()
                                && streams.is_empty(),
                            "discover bootstrap must not create session state"
                        );
                        uses_modern_http = false;

                        let response = self
                            .client
                            .post_message_with_max_sse_event_size(
                                config.uri.clone(),
                                message,
                                None,
                                config.auth_header.clone(),
                                config.custom_headers.clone(),
                                config.max_sse_event_size,
                            )
                            .await;
                        let response = match response {
                            Ok(response) => {
                                let _ = responder.send(Ok(()));
                                response
                            }
                            Err(error) => {
                                let _ = responder.send(Err(error));
                                continue;
                            }
                        };
                        let (initialize_response, new_session_id) = response
                            .expect_initialized::<C::Error>()
                            .await
                            .map_err(WorkerQuitReason::fatal_context(
                                "process fallback initialize response",
                            ))?;
                        session_id = new_session_id.map(Arc::from);
                        if session_id.is_none() && !config.allow_stateless {
                            return Err(WorkerQuitReason::fatal(
                                StreamableHttpError::<C::Error>::MissingSessionIdInResponse,
                                "process fallback initialize response",
                            ));
                        }
                        (negotiated_version, protocol_headers) = negotiate_version_headers(
                            &initialize_response,
                            config.custom_headers.clone(),
                        );
                        session_cleanup_info =
                            session_id.as_ref().map(|session_id| SessionCleanupInfo {
                                client: self.client.clone(),
                                uri: config.uri.clone(),
                                session_id: session_id.clone(),
                                auth_header: config.auth_header.clone(),
                                protocol_headers: protocol_headers.clone(),
                            });
                        context.send_to_handler(initialize_response).await?;
                        awaiting_fallback_initialized = true;
                        continue;
                    }

                    let request_id = Self::client_request_id(&message);
                    let inline_version = match &message {
                        ClientJsonRpcMessage::Request(request) => {
                            request.request.get_meta().protocol_version()
                        }
                        _ => None,
                    };
                    let is_initialized_notification = matches!(
                        &message,
                        ClientJsonRpcMessage::Notification(notification)
                            if matches!(
                                &notification.notification,
                                ClientNotification::InitializedNotification(_)
                            )
                    );
                    // Pass a clone to the first attempt so `message` is retained for a
                    // potential re-init retry. `post_message` takes ownership and the
                    // trait cannot be changed, so the clone is unavoidable.
                    let (request_version, request_headers) = request_version_headers(
                        &protocol_headers,
                        &message,
                        &negotiated_version,
                        &tool_header_cache,
                    );
                    if inline_version.is_some() {
                        negotiated_version = request_version.clone();
                        if let Ok(value) = HeaderValue::from_str(request_version.as_str()) {
                            protocol_headers
                                .insert(HeaderName::from_static("mcp-protocol-version"), value);
                        }
                        if let Some(cleanup) = &mut session_cleanup_info {
                            cleanup.protocol_headers = protocol_headers.clone();
                        }
                    }
                    let response = self
                        .client
                        .post_message_with_max_sse_event_size(
                            config.uri.clone(),
                            message.clone(),
                            session_id.clone(),
                            config.auth_header.clone(),
                            request_headers,
                            config.max_sse_event_size,
                        )
                        .await;
                    let send_result = match response {
                        Err(StreamableHttpError::SessionExpired) => {
                            if let Some(saved_init_request) = saved_init_request
                                .as_ref()
                                .filter(|_| config.reinit_on_expired_session)
                            {
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
                                    config.max_sse_event_size,
                                )
                                .await
                                {
                                    Ok((
                                        new_session_id,
                                        new_negotiated_version,
                                        new_protocol_headers,
                                    )) => {
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
                                        negotiated_version = new_negotiated_version;
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
                                            Self::spawn_common_stream(
                                                &mut streams,
                                                self.client.clone(),
                                                new_sid.clone(),
                                                &config,
                                                protocol_headers.clone(),
                                                sse_worker_tx.clone(),
                                                transport_task_ct.clone(),
                                            );
                                        }

                                        let (_, retry_headers) = request_version_headers(
                                            &protocol_headers,
                                            &message,
                                            &negotiated_version,
                                            &tool_header_cache,
                                        );
                                        let retry_response = self
                                            .client
                                            .post_message_with_max_sse_event_size(
                                                config.uri.clone(),
                                                message,
                                                session_id.clone(),
                                                config.auth_header.clone(),
                                                retry_headers,
                                                config.max_sse_event_size,
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
                                            Ok(StreamableHttpPostResponse::Json(mut msg, ..)) => {
                                                cache_tools_from_response(
                                                    &mut tool_header_cache,
                                                    &mut msg,
                                                    &negotiated_version,
                                                );
                                                context.send_to_handler(msg).await?;
                                                Ok(())
                                            }
                                            Ok(StreamableHttpPostResponse::Sse(stream, ..)) => {
                                                let stream_request_id = request_id.clone();
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
                                                    config.max_sse_event_size,
                                                    self.config.retry_config.clone(),
                                                );
                                                let stream_ct = transport_task_ct.child_token();
                                                if uses_modern_http
                                                    && let Some(request_id) =
                                                        stream_request_id.as_ref()
                                                {
                                                    request_stream_cancellations.insert(
                                                        request_id.clone(),
                                                        stream_ct.clone(),
                                                    );
                                                }
                                                let stream_tx = sse_worker_tx.clone();
                                                streams.spawn(async move {
                                                    let result = Self::execute_sse_stream(
                                                        sse_stream, stream_tx, true, stream_ct,
                                                    )
                                                    .await;
                                                    (stream_request_id, result)
                                                });
                                                tracing::trace!("got new sse stream after re-init");
                                                Ok(())
                                            }
                                        }
                                    }
                                    Err(reinit_err) => Err(reinit_err),
                                }
                            } else {
                                Err(StreamableHttpError::SessionExpired)
                            }
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
                        Ok(StreamableHttpPostResponse::Json(mut message, ..)) => {
                            cache_tools_from_response(
                                &mut tool_header_cache,
                                &mut message,
                                &negotiated_version,
                            );
                            context.send_to_handler(message).await?;
                            Ok(())
                        }
                        Ok(StreamableHttpPostResponse::Sse(stream, ..)) => {
                            let stream_request_id = request_id.clone();
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
                                config.max_sse_event_size,
                                self.config.retry_config.clone(),
                            );
                            let stream_ct = transport_task_ct.child_token();
                            if uses_modern_http && let Some(request_id) = stream_request_id.as_ref()
                            {
                                request_stream_cancellations
                                    .insert(request_id.clone(), stream_ct.clone());
                            }
                            let stream_tx = sse_worker_tx.clone();
                            streams.spawn(async move {
                                let result = Self::execute_sse_stream(
                                    sse_stream, stream_tx, true, stream_ct,
                                )
                                .await;
                                (stream_request_id, result)
                            });
                            tracing::trace!("got new sse stream");
                            Ok(())
                        }
                    };
                    if send_result.is_ok()
                        && awaiting_fallback_initialized
                        && is_initialized_notification
                    {
                        if let Some(session_id) = &session_id {
                            Self::spawn_common_stream(
                                &mut streams,
                                self.client.clone(),
                                session_id.clone(),
                                &config,
                                protocol_headers.clone(),
                                sse_worker_tx.clone(),
                                transport_task_ct.clone(),
                            );
                        }
                        awaiting_fallback_initialized = false;
                    }
                    let _ = responder.send(send_result);
                }
                Event::ServerMessage(mut json_rpc_message) => {
                    if let Some(response_id) = Self::server_response_id(&json_rpc_message)
                        && let Some(stream_ct) = crate::service::remove_pending_request(
                            &mut request_stream_cancellations,
                            response_id,
                        )
                    {
                        stream_ct.cancel();
                    }
                    Self::clear_stream_response_pending(
                        &mut pending_stream_response_ids,
                        &json_rpc_message,
                    );
                    cache_tools_from_response(
                        &mut tool_header_cache,
                        &mut json_rpc_message,
                        &negotiated_version,
                    );
                    // send the message to the handler
                    if let Err(e) = context.send_to_handler(json_rpc_message).await {
                        break 'main_loop Err(e);
                    }
                }
                Event::StreamResult { request_id, result } => {
                    if let Some(request_id) = request_id {
                        Self::drain_queued_stream_messages(
                            &mut sse_worker_rx,
                            &mut context,
                            &mut pending_stream_response_ids,
                        )
                        .await?;
                        request_stream_cancellations.remove(&request_id);
                        if pending_stream_response_ids.remove(&request_id) {
                            context
                                .send_to_handler(ServerJsonRpcMessage::error(
                                    ErrorData::transport_closed(
                                        "streamable HTTP response stream closed before its final response",
                                    ),
                                    Some(request_id),
                                ))
                                .await?;
                        }
                    }
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
///         _session_id: Option<Arc<str>>,
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
    ///         _session_id: Option<Arc<str>>,
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
    /// Maximum raw size of one SSE event accepted from the server.
    ///
    /// The built-in reqwest and Unix socket clients enforce this value. Custom
    /// [`StreamableHttpClient`] implementations must override the corresponding
    /// `*_with_max_sse_event_size` methods to enforce it.
    pub max_sse_event_size: usize,
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

    /// Set the maximum raw size of one SSE event accepted from the server.
    pub fn max_sse_event_size(mut self, bytes: usize) -> Self {
        self.max_sse_event_size = bytes;
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
            max_sse_event_size: DEFAULT_MAX_SSE_EVENT_SIZE,
            reinit_on_expired_session: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use serde_json::json;

    use super::*;
    use crate::model::{ListToolsResult, NumberOrString, ServerResult, Tool};

    type ReconnectAttempt = (Option<String>, Option<String>);

    #[derive(Clone, Default)]
    struct StatelessReconnectClient {
        reconnects: Arc<Mutex<Vec<ReconnectAttempt>>>,
    }

    impl StreamableHttpClient for StatelessReconnectClient {
        type Error = std::io::Error;

        async fn post_message(
            &self,
            _uri: Arc<str>,
            _message: ClientJsonRpcMessage,
            _session_id: Option<Arc<str>>,
            _auth_header: Option<String>,
            _custom_headers: HashMap<HeaderName, HeaderValue>,
        ) -> Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>> {
            Err(StreamableHttpError::UnexpectedServerResponse(
                "unexpected POST".into(),
            ))
        }

        async fn delete_session(
            &self,
            _uri: Arc<str>,
            _session_id: Arc<str>,
            _auth_header: Option<String>,
            _custom_headers: HashMap<HeaderName, HeaderValue>,
        ) -> Result<(), StreamableHttpError<Self::Error>> {
            Ok(())
        }

        async fn get_stream(
            &self,
            _uri: Arc<str>,
            session_id: Option<Arc<str>>,
            last_event_id: Option<String>,
            _auth_header: Option<String>,
            _custom_headers: HashMap<HeaderName, HeaderValue>,
        ) -> Result<BoxedSseStream, StreamableHttpError<Self::Error>> {
            self.reconnects
                .lock()
                .expect("lock reconnects")
                .push((session_id.map(|id| id.to_string()), last_event_id));
            let response = ServerJsonRpcMessage::response(
                ServerResult::ListToolsResult(ListToolsResult::default()),
                NumberOrString::Number(1),
            );
            Ok(futures::stream::once(async move {
                Ok(Sse {
                    event: None,
                    data: Some(serde_json::to_string(&response).expect("serialize response")),
                    id: Some("event-1".into()),
                    retry: None,
                })
            })
            .boxed())
        }
    }

    #[tokio::test]
    async fn stateless_response_reconnects_with_last_event_id() {
        let initial = futures::stream::iter([Ok(Sse {
            event: None,
            data: None,
            id: Some("event-0".into()),
            retry: Some(0),
        })])
        .boxed();
        let client = StatelessReconnectClient::default();
        let reconnects = client.reconnects.clone();
        let stream =
            StreamableHttpClientWorker::<StatelessReconnectClient>::response_sse_to_jsonrpc(
                initial,
                None,
                client,
                Arc::from("http://localhost/mcp"),
                None,
                HashMap::new(),
                DEFAULT_MAX_SSE_EVENT_SIZE,
                Arc::new(ExponentialBackoff {
                    max_times: Some(1),
                    base_duration: Duration::ZERO,
                }),
            );
        let mut stream = std::pin::pin!(stream);

        let message = stream.next().await.expect("replayed response").unwrap();

        assert!(matches!(message, ServerJsonRpcMessage::Response(_)));
        assert_eq!(
            reconnects.lock().expect("lock reconnects").as_slice(),
            &[(None, Some("event-0".into()))]
        );
    }

    fn tool(name: &'static str, annotation: serde_json::Value) -> Tool {
        let schema = json!({
            "type": "object",
            "properties": {
                "value": annotation,
            },
        });
        Tool::new(
            name,
            name,
            Arc::new(schema.as_object().expect("object schema").clone()),
        )
    }

    #[test]
    fn cache_tools_removes_invalid_header_annotations() {
        let valid = tool(
            "valid",
            json!({ "type": "string", "x-mcp-header": "Value" }),
        );
        let invalid = tool("invalid", json!({ "type": "string", "x-mcp-header": "" }));
        let mut message = ServerJsonRpcMessage::response(
            ServerResult::ListToolsResult(ListToolsResult::with_all_items(vec![valid, invalid])),
            NumberOrString::Number(1),
        );
        let mut cache = HashMap::new();

        cache_tools_from_response(&mut cache, &mut message, &ProtocolVersion::V_2026_07_28);

        let ServerJsonRpcMessage::Response(response) = &mut message else {
            panic!("expected tools/list response");
        };
        let ServerResult::ListToolsResult(result) = &mut response.result else {
            panic!("expected tools/list result");
        };
        assert_eq!(
            (
                result
                    .tools
                    .iter()
                    .map(|tool| tool.name.as_ref())
                    .collect::<Vec<_>>(),
                cache.keys().map(String::as_str).collect::<Vec<_>>(),
            ),
            (vec!["valid"], vec!["valid"])
        );
    }

    #[test]
    fn cache_tools_preserves_pre_standard_header_results() {
        let invalid = tool("legacy", json!({ "type": "string", "x-mcp-header": "" }));
        let mut message = ServerJsonRpcMessage::response(
            ServerResult::ListToolsResult(ListToolsResult::with_all_items(vec![invalid])),
            NumberOrString::Number(1),
        );
        let mut cache = HashMap::new();

        cache_tools_from_response(&mut cache, &mut message, &ProtocolVersion::V_2025_11_25);

        let ServerJsonRpcMessage::Response(response) = message else {
            panic!("expected tools/list response");
        };
        let ServerResult::ListToolsResult(result) = response.result else {
            panic!("expected tools/list result");
        };
        assert_eq!(
            result
                .tools
                .iter()
                .map(|tool| tool.name.as_ref())
                .collect::<Vec<_>>(),
            vec!["legacy"]
        );
    }

    #[cfg(feature = "transport-streamable-http-client-reqwest")]
    #[test]
    fn clear_stream_response_pending_accepts_stringified_numeric_id() {
        let mut pending = HashSet::from([NumberOrString::Number(1)]);
        let response = ServerJsonRpcMessage::response(
            ServerResult::ListToolsResult(ListToolsResult::default()),
            NumberOrString::String("1".into()),
        );

        StreamableHttpClientWorker::<reqwest::Client>::clear_stream_response_pending(
            &mut pending,
            &response,
        );

        assert!(pending.is_empty());
    }

    #[cfg(feature = "transport-streamable-http-client-reqwest")]
    #[test]
    fn clear_stream_response_pending_prefers_exact_string_id() {
        let string_id = NumberOrString::String("1".into());
        let mut pending = HashSet::from([NumberOrString::Number(1), string_id.clone()]);
        let response = ServerJsonRpcMessage::response(
            ServerResult::ListToolsResult(ListToolsResult::default()),
            string_id,
        );

        StreamableHttpClientWorker::<reqwest::Client>::clear_stream_response_pending(
            &mut pending,
            &response,
        );

        assert_eq!(pending, HashSet::from([NumberOrString::Number(1)]));
    }
}
