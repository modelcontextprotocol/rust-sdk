use std::{convert::Infallible, fmt::Display, sync::Arc, time::Duration};

use bytes::Bytes;
use futures::{StreamExt, future::BoxFuture};
use http::{HeaderMap, Method, Request, Response, header::ALLOW};
use http_body::Body;
use http_body_util::{BodyExt, Full, combinators::BoxBody};
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;

use super::session::SessionManager;
use crate::{
    RoleServer,
    model::{ClientJsonRpcMessage, ClientRequest, GetExtensions, ProtocolVersion},
    serve_server,
    service::serve_directly,
    transport::{
        OneshotTransport, TransportAdapterIdentity,
        common::{
            http_header::{
                EVENT_STREAM_MIME_TYPE, HEADER_LAST_EVENT_ID, HEADER_MCP_PROTOCOL_VERSION,
                HEADER_SESSION_ID, JSON_MIME_TYPE,
            },
            server_side_http::{
                BoxResponse, ServerSseMessage, accepted_response, expect_json,
                internal_error_response, sse_stream_response, unexpected_message_response,
            },
        },
    },
};

#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct StreamableHttpServerConfig {
    /// The ping message duration for SSE connections.
    pub sse_keep_alive: Option<Duration>,
    /// The retry interval for SSE priming events.
    pub sse_retry: Option<Duration>,
    /// If true, the server will create a session for each request and keep it alive.
    /// When enabled, SSE priming events are sent to enable client reconnection.
    pub stateful_mode: bool,
    /// When true and `stateful_mode` is false, the server returns
    /// `Content-Type: application/json` directly instead of `text/event-stream`.
    /// This eliminates SSE framing overhead for simple request-response tools,
    /// allowed by the MCP Streamable HTTP spec (2025-06-18).
    pub json_response: bool,
    /// Cancellation token for the Streamable HTTP server.
    ///
    /// When this token is cancelled, all active sessions are terminated and
    /// the server stops accepting new requests.
    pub cancellation_token: CancellationToken,
    /// Allowed hostnames or `host:port` authorities for inbound `Host` validation.
    ///
    /// By default, Streamable HTTP servers only accept loopback hosts to
    /// prevent DNS rebinding attacks against locally running servers. Public
    /// deployments should override this list with their own hostnames.
    /// examples:
    ///     allowed_hosts = ["localhost", "127.0.0.1", "0.0.0.0"]
    /// or with ports:
    ///     allowed_hosts = ["example.com", "example.com:8080"]
    pub allowed_hosts: Vec<String>,
}

impl Default for StreamableHttpServerConfig {
    fn default() -> Self {
        Self {
            sse_keep_alive: Some(Duration::from_secs(15)),
            sse_retry: Some(Duration::from_secs(3)),
            stateful_mode: true,
            json_response: false,
            cancellation_token: CancellationToken::new(),
            allowed_hosts: vec!["localhost".into(), "127.0.0.1".into(), "::1".into()],
        }
    }
}

impl StreamableHttpServerConfig {
    pub fn with_allowed_hosts(
        mut self,
        allowed_hosts: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.allowed_hosts = allowed_hosts.into_iter().map(Into::into).collect();
        self
    }
    /// Disable allowed hosts. This will allow requests with any `Host` header, which is NOT recommended for public deployments.
    pub fn disable_allowed_hosts(mut self) -> Self {
        self.allowed_hosts.clear();
        self
    }
    pub fn with_sse_keep_alive(mut self, duration: Option<Duration>) -> Self {
        self.sse_keep_alive = duration;
        self
    }

    pub fn with_sse_retry(mut self, duration: Option<Duration>) -> Self {
        self.sse_retry = duration;
        self
    }

    pub fn with_stateful_mode(mut self, stateful: bool) -> Self {
        self.stateful_mode = stateful;
        self
    }

    pub fn with_json_response(mut self, json_response: bool) -> Self {
        self.json_response = json_response;
        self
    }

    pub fn with_cancellation_token(mut self, token: CancellationToken) -> Self {
        self.cancellation_token = token;
        self
    }
}

#[expect(
    clippy::result_large_err,
    reason = "BoxResponse is intentionally large; matches other handlers in this file"
)]
/// Validates the `MCP-Protocol-Version` header on incoming HTTP requests.
///
/// Per the MCP 2025-06-18 spec:
/// - If the header is present but contains an unsupported version, return 400 Bad Request.
/// - If the header is absent, assume `2025-03-26` for backwards compatibility (no error).
fn validate_protocol_version_header(headers: &http::HeaderMap) -> Result<(), BoxResponse> {
    if let Some(value) = headers.get(HEADER_MCP_PROTOCOL_VERSION) {
        let version_str = value.to_str().map_err(|_| {
            Response::builder()
                .status(http::StatusCode::BAD_REQUEST)
                .body(
                    Full::new(Bytes::from(
                        "Bad Request: Invalid MCP-Protocol-Version header encoding",
                    ))
                    .boxed(),
                )
                .expect("valid response")
        })?;
        let is_known = ProtocolVersion::KNOWN_VERSIONS
            .iter()
            .any(|v| v.as_str() == version_str);
        if !is_known {
            return Err(Response::builder()
                .status(http::StatusCode::BAD_REQUEST)
                .body(
                    Full::new(Bytes::from(format!(
                        "Bad Request: Unsupported MCP-Protocol-Version: {version_str}"
                    )))
                    .boxed(),
                )
                .expect("valid response"));
        }
    }
    Ok(())
}

fn forbidden_response(message: impl Into<String>) -> BoxResponse {
    Response::builder()
        .status(http::StatusCode::FORBIDDEN)
        .body(Full::new(Bytes::from(message.into())).boxed())
        .expect("valid response")
}

fn normalize_host(host: &str) -> String {
    host.trim_matches('[')
        .trim_matches(']')
        .to_ascii_lowercase()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NormalizedAuthority {
    host: String,
    port: Option<u16>,
}

fn normalize_authority(host: &str, port: Option<u16>) -> NormalizedAuthority {
    NormalizedAuthority {
        host: normalize_host(host),
        port,
    }
}

fn parse_allowed_authority(allowed: &str) -> Option<NormalizedAuthority> {
    let allowed = allowed.trim();
    if allowed.is_empty() {
        return None;
    }

    if let Ok(authority) = http::uri::Authority::try_from(allowed) {
        return Some(normalize_authority(authority.host(), authority.port_u16()));
    }

    Some(normalize_authority(allowed, None))
}

fn host_is_allowed(host: &NormalizedAuthority, allowed_hosts: &[String]) -> bool {
    if allowed_hosts.is_empty() {
        // If the allowed hosts list is empty, allow all hosts (not recommended).
        return true;
    }
    allowed_hosts
        .iter()
        .filter_map(|allowed| parse_allowed_authority(allowed))
        .any(|allowed| {
            allowed.host == host.host
                && match allowed.port {
                    Some(port) => host.port == Some(port),
                    None => true,
                }
        })
}

fn bad_request_response(message: &str) -> BoxResponse {
    let body = Full::from(message.to_string()).boxed();

    http::Response::builder()
        .status(http::StatusCode::BAD_REQUEST)
        .header(http::header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(body)
        .expect("failed to build bad request response")
}

fn parse_host_header(headers: &HeaderMap) -> Result<NormalizedAuthority, BoxResponse> {
    let Some(host) = headers.get(http::header::HOST) else {
        return Err(bad_request_response("Bad Request: missing Host header"));
    };

    let host = host
        .to_str()
        .map_err(|_| bad_request_response("Bad Request: Invalid Host header encoding"))?;
    let authority = http::uri::Authority::try_from(host)
        .map_err(|_| bad_request_response("Bad Request: Invalid Host header"))?;
    Ok(normalize_authority(authority.host(), authority.port_u16()))
}

fn validate_dns_rebinding_headers(
    headers: &HeaderMap,
    config: &StreamableHttpServerConfig,
) -> Result<(), BoxResponse> {
    let host = parse_host_header(headers)?;
    if !host_is_allowed(&host, &config.allowed_hosts) {
        return Err(forbidden_response("Forbidden: Host header is not allowed"));
    }

    Ok(())
}

/// # Streamable HTTP server
///
/// An HTTP service that implements the
/// [Streamable HTTP transport](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports#streamable-http)
/// for MCP servers.
///
/// ## Session management
///
/// When [`StreamableHttpServerConfig::stateful_mode`] is `true` (the default),
/// the server creates a session for each client that sends an `initialize`
/// request. The session ID is returned in the `Mcp-Session-Id` response header
/// and the client must include it on all subsequent requests.
///
/// Two tool calls carrying the same `Mcp-Session-Id` come from the same logical
/// session (typically one conversation in an LLM client). Different session IDs
/// mean different sessions.
///
/// The [`SessionManager`] trait controls how sessions are stored and routed:
///
/// * [`LocalSessionManager`](super::session::local::LocalSessionManager) —
///   in-memory session store (default).
/// * [`NeverSessionManager`](super::session::never::NeverSessionManager) —
///   disables sessions entirely (stateless mode).
///
/// ## Accessing HTTP request data from tool handlers
///
/// The service consumes the request body but injects the remaining
/// [`http::request::Parts`] into [`crate::model::Extensions`], which is
/// accessible through [`crate::service::RequestContext`].
///
/// ### Reading the raw HTTP parts
///
/// ```rust
/// use rmcp::handler::server::tool::Extension;
/// use http::request::Parts;
/// async fn my_tool(Extension(parts): Extension<Parts>) {
///     tracing::info!("http parts:{parts:?}")
/// }
/// ```
///
/// ### Reading the session ID inside a tool handler
///
/// ```rust,ignore
/// use rmcp::handler::server::tool::Extension;
/// use rmcp::service::RequestContext;
/// use rmcp::model::RoleServer;
///
/// #[tool(description = "session-aware tool")]
/// async fn my_tool(
///     &self,
///     Extension(parts): Extension<http::request::Parts>,
/// ) -> Result<CallToolResult, rmcp::ErrorData> {
///     if let Some(session_id) = parts.headers.get("mcp-session-id") {
///         tracing::info!(?session_id, "called from session");
///     }
///     // ...
///     # todo!()
/// }
/// ```
///
/// ### Accessing custom axum/tower extension state
///
/// State added via axum's `Extension` layer is available inside
/// `Parts.extensions`:
///
/// ```rust,ignore
/// use rmcp::service::RequestContext;
/// use rmcp::model::RoleServer;
///
/// #[derive(Clone)]
/// struct AppState { /* ... */ }
///
/// #[tool(description = "example")]
/// async fn my_tool(
///     &self,
///     ctx: RequestContext<RoleServer>,
/// ) -> Result<CallToolResult, rmcp::ErrorData> {
///     let parts = ctx.extensions.get::<http::request::Parts>().unwrap();
///     let state = parts.extensions.get::<AppState>().unwrap();
///     // use state...
///     # todo!()
/// }
/// ```
pub struct StreamableHttpService<S, M> {
    pub config: StreamableHttpServerConfig,
    session_manager: Arc<M>,
    service_factory: Arc<dyn Fn() -> Result<S, std::io::Error> + Send + Sync>,
}

impl<S, M> Clone for StreamableHttpService<S, M> {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            session_manager: self.session_manager.clone(),
            service_factory: self.service_factory.clone(),
        }
    }
}

impl<RequestBody, S, M> tower_service::Service<Request<RequestBody>> for StreamableHttpService<S, M>
where
    RequestBody: Body + Send + 'static,
    S: crate::Service<RoleServer> + Send + 'static,
    M: SessionManager,
    RequestBody::Error: Display,
    RequestBody::Data: Send + 'static,
{
    type Response = BoxResponse;
    type Error = Infallible;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;
    fn call(&mut self, req: http::Request<RequestBody>) -> Self::Future {
        let service = self.clone();
        Box::pin(async move {
            let response = service.handle(req).await;
            Ok(response)
        })
    }
    fn poll_ready(
        &mut self,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        std::task::Poll::Ready(Ok(()))
    }
}

impl<S, M> StreamableHttpService<S, M>
where
    S: crate::Service<RoleServer> + Send + 'static,
    M: SessionManager,
{
    pub fn new(
        service_factory: impl Fn() -> Result<S, std::io::Error> + Send + Sync + 'static,
        session_manager: Arc<M>,
        config: StreamableHttpServerConfig,
    ) -> Self {
        Self {
            config,
            session_manager,
            service_factory: Arc::new(service_factory),
        }
    }
    fn get_service(&self) -> Result<S, std::io::Error> {
        (self.service_factory)()
    }
    pub async fn handle<B>(&self, request: Request<B>) -> Response<BoxBody<Bytes, Infallible>>
    where
        B: Body + Send + 'static,
        B::Error: Display,
    {
        if let Err(response) = validate_dns_rebinding_headers(request.headers(), &self.config) {
            return response;
        }
        let method = request.method().clone();
        let allowed_methods = match self.config.stateful_mode {
            true => "GET, POST, DELETE",
            false => "POST",
        };
        let result = match (method, self.config.stateful_mode) {
            (Method::POST, _) => self.handle_post(request).await,
            // if we're not in stateful mode, we don't support GET or DELETE because there is no session
            (Method::GET, true) => self.handle_get(request).await,
            (Method::DELETE, true) => self.handle_delete(request).await,
            _ => {
                // Handle other methods or return an error
                let response = Response::builder()
                    .status(http::StatusCode::METHOD_NOT_ALLOWED)
                    .header(ALLOW, allowed_methods)
                    .body(Full::new(Bytes::from("Method Not Allowed")).boxed())
                    .expect("valid response");
                return response;
            }
        };
        match result {
            Ok(response) => response,
            Err(response) => response,
        }
    }
    async fn handle_get<B>(&self, request: Request<B>) -> Result<BoxResponse, BoxResponse>
    where
        B: Body + Send + 'static,
        B::Error: Display,
    {
        // check accept header
        if !request
            .headers()
            .get(http::header::ACCEPT)
            .and_then(|header| header.to_str().ok())
            .is_some_and(|header| header.contains(EVENT_STREAM_MIME_TYPE))
        {
            return Ok(Response::builder()
                .status(http::StatusCode::NOT_ACCEPTABLE)
                .body(
                    Full::new(Bytes::from(
                        "Not Acceptable: Client must accept text/event-stream",
                    ))
                    .boxed(),
                )
                .expect("valid response"));
        }
        // check session id
        let session_id = request
            .headers()
            .get(HEADER_SESSION_ID)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_owned().into());
        let Some(session_id) = session_id else {
            // MCP spec: servers that require a session ID SHOULD respond with 400 Bad Request
            return Ok(Response::builder()
                .status(http::StatusCode::BAD_REQUEST)
                .body(Full::new(Bytes::from("Bad Request: Session ID is required")).boxed())
                .expect("valid response"));
        };
        // check if session exists
        let has_session = self
            .session_manager
            .has_session(&session_id)
            .await
            .map_err(internal_error_response("check session"))?;
        if !has_session {
            // MCP spec: server MUST respond with 404 Not Found for terminated/unknown sessions
            return Ok(Response::builder()
                .status(http::StatusCode::NOT_FOUND)
                .body(Full::new(Bytes::from("Not Found: Session not found")).boxed())
                .expect("valid response"));
        }
        // Validate MCP-Protocol-Version header (per 2025-06-18 spec)
        validate_protocol_version_header(request.headers())?;
        // check if last event id is provided
        let last_event_id = request
            .headers()
            .get(HEADER_LAST_EVENT_ID)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_owned());
        if let Some(last_event_id) = last_event_id {
            // check if session has this event id
            let stream = self
                .session_manager
                .resume(&session_id, last_event_id)
                .await
                .map_err(internal_error_response("resume session"))?;
            // Resume doesn't need priming - client already has the event ID
            Ok(sse_stream_response(
                stream,
                self.config.sse_keep_alive,
                self.config.cancellation_token.child_token(),
            ))
        } else {
            // create standalone stream
            let stream = self
                .session_manager
                .create_standalone_stream(&session_id)
                .await
                .map_err(internal_error_response("create standalone stream"))?;
            // Prepend priming event if sse_retry configured
            let stream = if let Some(retry) = self.config.sse_retry {
                let priming = ServerSseMessage::priming("0", retry);
                futures::stream::once(async move { priming })
                    .chain(stream)
                    .left_stream()
            } else {
                stream.right_stream()
            };
            Ok(sse_stream_response(
                stream,
                self.config.sse_keep_alive,
                self.config.cancellation_token.child_token(),
            ))
        }
    }

    async fn handle_post<B>(&self, request: Request<B>) -> Result<BoxResponse, BoxResponse>
    where
        B: Body + Send + 'static,
        B::Error: Display,
    {
        // check accept header
        if !request
            .headers()
            .get(http::header::ACCEPT)
            .and_then(|header| header.to_str().ok())
            .is_some_and(|header| {
                header.contains(JSON_MIME_TYPE) && header.contains(EVENT_STREAM_MIME_TYPE)
            })
        {
            return Ok(Response::builder()
                .status(http::StatusCode::NOT_ACCEPTABLE)
                .body(Full::new(Bytes::from("Not Acceptable: Client must accept both application/json and text/event-stream")).boxed())
                .expect("valid response"));
        }

        // check content type
        if !request
            .headers()
            .get(http::header::CONTENT_TYPE)
            .and_then(|header| header.to_str().ok())
            .is_some_and(|header| header.starts_with(JSON_MIME_TYPE))
        {
            return Ok(Response::builder()
                .status(http::StatusCode::UNSUPPORTED_MEDIA_TYPE)
                .body(
                    Full::new(Bytes::from(
                        "Unsupported Media Type: Content-Type must be application/json",
                    ))
                    .boxed(),
                )
                .expect("valid response"));
        }

        // json deserialize request body
        let (part, body) = request.into_parts();
        let mut message = match expect_json(body).await {
            Ok(message) => message,
            Err(response) => return Ok(response),
        };

        if self.config.stateful_mode {
            // do we have a session id?
            let session_id = part
                .headers
                .get(HEADER_SESSION_ID)
                .and_then(|v| v.to_str().ok());
            if let Some(session_id) = session_id {
                let session_id = session_id.to_owned().into();
                let has_session = self
                    .session_manager
                    .has_session(&session_id)
                    .await
                    .map_err(internal_error_response("check session"))?;
                if !has_session {
                    // MCP spec: server MUST respond with 404 Not Found for terminated/unknown sessions
                    return Ok(Response::builder()
                        .status(http::StatusCode::NOT_FOUND)
                        .body(Full::new(Bytes::from("Not Found: Session not found")).boxed())
                        .expect("valid response"));
                }

                // Validate MCP-Protocol-Version header (per 2025-06-18 spec)
                validate_protocol_version_header(&part.headers)?;

                // inject request part to extensions
                match &mut message {
                    ClientJsonRpcMessage::Request(req) => {
                        req.request.extensions_mut().insert(part);
                    }
                    ClientJsonRpcMessage::Notification(not) => {
                        not.notification.extensions_mut().insert(part);
                    }
                    _ => {
                        // skip
                    }
                }

                match message {
                    ClientJsonRpcMessage::Request(_) => {
                        let stream = self
                            .session_manager
                            .create_stream(&session_id, message)
                            .await
                            .map_err(internal_error_response("get session"))?;
                        // Prepend priming event if sse_retry configured
                        let stream = if let Some(retry) = self.config.sse_retry {
                            let priming = ServerSseMessage::priming("0", retry);
                            futures::stream::once(async move { priming })
                                .chain(stream)
                                .left_stream()
                        } else {
                            stream.right_stream()
                        };
                        Ok(sse_stream_response(
                            stream,
                            self.config.sse_keep_alive,
                            self.config.cancellation_token.child_token(),
                        ))
                    }
                    ClientJsonRpcMessage::Notification(_)
                    | ClientJsonRpcMessage::Response(_)
                    | ClientJsonRpcMessage::Error(_) => {
                        // handle notification
                        self.session_manager
                            .accept_message(&session_id, message)
                            .await
                            .map_err(internal_error_response("accept message"))?;
                        Ok(accepted_response())
                    }
                }
            } else {
                let (session_id, transport) = self
                    .session_manager
                    .create_session()
                    .await
                    .map_err(internal_error_response("create session"))?;
                if let ClientJsonRpcMessage::Request(req) = &mut message {
                    if !matches!(req.request, ClientRequest::InitializeRequest(_)) {
                        return Err(unexpected_message_response("initialize request"));
                    }
                    // inject request part to extensions
                    req.request.extensions_mut().insert(part);
                } else {
                    return Err(unexpected_message_response("initialize request"));
                }
                let service = self
                    .get_service()
                    .map_err(internal_error_response("get service"))?;
                // spawn a task to serve the session
                tokio::spawn({
                    let session_manager = self.session_manager.clone();
                    let session_id = session_id.clone();
                    async move {
                        let service = serve_server::<S, M::Transport, _, TransportAdapterIdentity>(
                            service, transport,
                        )
                        .await;
                        match service {
                            Ok(service) => {
                                // on service created
                                let _ = service.waiting().await;
                            }
                            Err(e) => {
                                tracing::error!("Failed to create service: {e}");
                            }
                        }
                        let _ = session_manager
                            .close_session(&session_id)
                            .await
                            .inspect_err(|e| {
                                tracing::error!("Failed to close session {session_id}: {e}");
                            });
                    }
                });
                // get initialize response
                let response = self
                    .session_manager
                    .initialize_session(&session_id, message)
                    .await
                    .map_err(internal_error_response("create stream"))?;
                let stream =
                    futures::stream::once(async move { ServerSseMessage::from_message(response) });
                // Prepend priming event if sse_retry configured
                let stream = if let Some(retry) = self.config.sse_retry {
                    let priming = ServerSseMessage::priming("0", retry);
                    futures::stream::once(async move { priming })
                        .chain(stream)
                        .left_stream()
                } else {
                    stream.right_stream()
                };
                let mut response = sse_stream_response(
                    stream,
                    self.config.sse_keep_alive,
                    self.config.cancellation_token.child_token(),
                );

                response.headers_mut().insert(
                    HEADER_SESSION_ID,
                    session_id
                        .parse()
                        .map_err(internal_error_response("create session id header"))?,
                );
                Ok(response)
            }
        } else {
            // Stateless mode: validate MCP-Protocol-Version on non-init requests
            let is_init = matches!(
                &message,
                ClientJsonRpcMessage::Request(req) if matches!(req.request, ClientRequest::InitializeRequest(_))
            );
            if !is_init {
                validate_protocol_version_header(&part.headers)?;
            }
            let service = self
                .get_service()
                .map_err(internal_error_response("get service"))?;
            match message {
                ClientJsonRpcMessage::Request(mut request) => {
                    request.request.extensions_mut().insert(part);
                    let (transport, mut receiver) =
                        OneshotTransport::<RoleServer>::new(ClientJsonRpcMessage::Request(request));
                    let service = serve_directly(service, transport, None);
                    tokio::spawn(async move {
                        // on service created
                        let _ = service.waiting().await;
                    });
                    if self.config.json_response {
                        // JSON-direct mode: await the single response and return as
                        // application/json, eliminating SSE framing overhead.
                        // Allowed by MCP Streamable HTTP spec (2025-06-18).
                        //
                        // Tools may emit progress notifications before their
                        // final response. In JSON-direct mode there is no
                        // secondary channel for those notifications, so keep
                        // draining until we receive the terminal response/error
                        // message that should satisfy the HTTP request.
                        let cancel = self.config.cancellation_token.child_token();
                        loop {
                            match tokio::select! {
                                res = receiver.recv() => res,
                                _ = cancel.cancelled() => None,
                            } {
                                Some(
                                    message @ (crate::model::ServerJsonRpcMessage::Response(_)
                                    | crate::model::ServerJsonRpcMessage::Error(_)),
                                ) => {
                                    tracing::trace!(?message);
                                    let body = serde_json::to_vec(&message).map_err(|e| {
                                        internal_error_response("serialize json response")(e)
                                    })?;
                                    break Ok(Response::builder()
                                        .status(http::StatusCode::OK)
                                        .header(http::header::CONTENT_TYPE, JSON_MIME_TYPE)
                                        .body(Full::new(Bytes::from(body)).boxed())
                                        .expect("valid response"));
                                }
                                Some(crate::model::ServerJsonRpcMessage::Notification(
                                    notification,
                                )) => {
                                    tracing::debug!(
                                        ?notification,
                                        "dropping server notification while awaiting JSON response"
                                    );
                                }
                                Some(crate::model::ServerJsonRpcMessage::Request(request)) => {
                                    tracing::warn!(
                                        ?request,
                                        "cannot deliver server request over JSON-direct response"
                                    );
                                    break Err(unexpected_message_response("response or error"));
                                }
                                None => {
                                    break Err(internal_error_response("empty response")(
                                        std::io::Error::new(
                                            std::io::ErrorKind::UnexpectedEof,
                                            "no response message received from handler",
                                        ),
                                    ));
                                }
                            }
                        }
                    } else {
                        // SSE mode (default): original behaviour preserved unchanged
                        let stream = ReceiverStream::new(receiver).map(|message| {
                            tracing::trace!(?message);
                            ServerSseMessage::from_message(message)
                        });
                        Ok(sse_stream_response(
                            stream,
                            self.config.sse_keep_alive,
                            self.config.cancellation_token.child_token(),
                        ))
                    }
                }
                ClientJsonRpcMessage::Notification(_notification) => {
                    // ignore
                    Ok(accepted_response())
                }
                ClientJsonRpcMessage::Response(_json_rpc_response) => Ok(accepted_response()),
                ClientJsonRpcMessage::Error(_json_rpc_error) => Ok(accepted_response()),
            }
        }
    }

    async fn handle_delete<B>(&self, request: Request<B>) -> Result<BoxResponse, BoxResponse>
    where
        B: Body + Send + 'static,
        B::Error: Display,
    {
        // check session id
        let session_id = request
            .headers()
            .get(HEADER_SESSION_ID)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_owned().into());
        let Some(session_id) = session_id else {
            // MCP spec: servers that require a session ID SHOULD respond with 400 Bad Request
            return Ok(Response::builder()
                .status(http::StatusCode::BAD_REQUEST)
                .body(Full::new(Bytes::from("Bad Request: Session ID is required")).boxed())
                .expect("valid response"));
        };
        // Validate MCP-Protocol-Version header (per 2025-06-18 spec)
        validate_protocol_version_header(request.headers())?;
        // close session
        self.session_manager
            .close_session(&session_id)
            .await
            .map_err(internal_error_response("close session"))?;
        Ok(accepted_response())
    }
}
