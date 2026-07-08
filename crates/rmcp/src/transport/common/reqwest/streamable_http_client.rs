use std::{borrow::Cow, collections::HashMap, sync::Arc};

use futures::{StreamExt, stream::BoxStream};
use http::{HeaderName, HeaderValue, header::WWW_AUTHENTICATE};
use reqwest::header::ACCEPT;
use sse_stream::{Sse, SseStream};

use crate::{
    model::{ClientJsonRpcMessage, JsonRpcMessage, ServerJsonRpcMessage},
    transport::{
        common::http_header::{
            EVENT_STREAM_MIME_TYPE, HEADER_LAST_EVENT_ID, HEADER_SESSION_ID, JSON_MIME_TYPE,
            extract_scope_from_header, validate_custom_header,
        },
        streamable_http_client::*,
    },
};

impl From<reqwest::Error> for StreamableHttpError<reqwest::Error> {
    fn from(e: reqwest::Error) -> Self {
        StreamableHttpError::Client(e)
    }
}

/// Applies custom headers to a request builder, rejecting reserved headers.
fn apply_custom_headers(
    mut builder: reqwest::RequestBuilder,
    custom_headers: HashMap<HeaderName, HeaderValue>,
) -> Result<reqwest::RequestBuilder, StreamableHttpError<reqwest::Error>> {
    for (name, value) in custom_headers {
        validate_custom_header(&name).map_err(StreamableHttpError::ReservedHeaderConflict)?;
        builder = builder.header(name, value);
    }
    Ok(builder)
}

/// Attempts to parse `body` as a JSON-RPC error message.
/// Returns `None` if the body is not parseable or is not a `JsonRpcMessage::Error`.
fn parse_json_rpc_error(body: &str) -> Option<ServerJsonRpcMessage> {
    match serde_json::from_str::<ServerJsonRpcMessage>(body) {
        Ok(message @ JsonRpcMessage::Error(_)) => Some(message),
        _ => None,
    }
}

impl StreamableHttpClient for reqwest::Client {
    type Error = reqwest::Error;

    async fn get_stream(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        last_event_id: Option<String>,
        auth_token: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<BoxStream<'static, Result<Sse, SseError>>, StreamableHttpError<Self::Error>> {
        let mut request_builder = self
            .get(uri.as_ref())
            .header(ACCEPT, [EVENT_STREAM_MIME_TYPE, JSON_MIME_TYPE].join(", "))
            .header(HEADER_SESSION_ID, session_id.as_ref());
        if let Some(last_event_id) = last_event_id {
            request_builder = request_builder.header(HEADER_LAST_EVENT_ID, last_event_id);
        }
        if let Some(auth_header) = auth_token {
            request_builder = request_builder.bearer_auth(auth_header);
        }
        request_builder = apply_custom_headers(request_builder, custom_headers)?;
        let response = request_builder.send().await?;
        if response.status() == reqwest::StatusCode::METHOD_NOT_ALLOWED {
            return Err(StreamableHttpError::ServerDoesNotSupportSse);
        }
        let response = response.error_for_status()?;
        match response.headers().get(reqwest::header::CONTENT_TYPE) {
            Some(ct) => {
                if !ct.as_bytes().starts_with(EVENT_STREAM_MIME_TYPE.as_bytes())
                    && !ct.as_bytes().starts_with(JSON_MIME_TYPE.as_bytes())
                {
                    return Err(StreamableHttpError::UnexpectedContentType(Some(
                        String::from_utf8_lossy(ct.as_bytes()).to_string(),
                    )));
                }
            }
            None => {
                return Err(StreamableHttpError::UnexpectedContentType(None));
            }
        }
        let event_stream = SseStream::from_bytes_stream(response.bytes_stream()).boxed();
        Ok(event_stream)
    }

    async fn delete_session(
        &self,
        uri: Arc<str>,
        session: Arc<str>,
        auth_token: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<(), StreamableHttpError<Self::Error>> {
        let mut request_builder = self.delete(uri.as_ref());
        if let Some(auth_header) = auth_token {
            request_builder = request_builder.bearer_auth(auth_header);
        }
        request_builder = request_builder.header(HEADER_SESSION_ID, session.as_ref());
        request_builder = apply_custom_headers(request_builder, custom_headers)?;
        let response = request_builder.send().await?;

        // if method no allowed
        if response.status() == reqwest::StatusCode::METHOD_NOT_ALLOWED {
            tracing::debug!("this server doesn't support deleting session");
            return Ok(());
        }
        let _response = response.error_for_status()?;
        Ok(())
    }

    async fn post_message(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_token: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>> {
        let mut request = self
            .post(uri.as_ref())
            .header(ACCEPT, [EVENT_STREAM_MIME_TYPE, JSON_MIME_TYPE].join(", "));
        if let Some(auth_header) = auth_token {
            request = request.bearer_auth(auth_header);
        }

        request = apply_custom_headers(request, custom_headers)?;
        let session_was_attached = session_id.is_some();
        if let Some(session_id) = session_id {
            request = request.header(HEADER_SESSION_ID, session_id.as_ref());
        }
        let response = request.json(&message).send().await?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            if let Some(header) = response.headers().get(WWW_AUTHENTICATE) {
                let header = header
                    .to_str()
                    .map_err(|_| {
                        StreamableHttpError::UnexpectedServerResponse(Cow::from(
                            "invalid www-authenticate header value",
                        ))
                    })?
                    .to_string();
                return Err(StreamableHttpError::AuthRequired(AuthRequiredError {
                    www_authenticate_header: header,
                }));
            }
        }
        if response.status() == reqwest::StatusCode::FORBIDDEN {
            if let Some(header) = response.headers().get(WWW_AUTHENTICATE) {
                let header_str = header.to_str().map_err(|_| {
                    StreamableHttpError::UnexpectedServerResponse(Cow::from(
                        "invalid www-authenticate header value",
                    ))
                })?;
                let scope = extract_scope_from_header(header_str);
                return Err(StreamableHttpError::InsufficientScope(
                    InsufficientScopeError {
                        www_authenticate_header: header_str.to_string(),
                        required_scope: scope,
                    },
                ));
            }
        }
        let status = response.status();
        if matches!(
            status,
            reqwest::StatusCode::ACCEPTED | reqwest::StatusCode::NO_CONTENT
        ) {
            return Ok(StreamableHttpPostResponse::Accepted);
        }
        if status == reqwest::StatusCode::NOT_FOUND && session_was_attached {
            return Err(StreamableHttpError::SessionExpired);
        }
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .map(|ct| String::from_utf8_lossy(ct.as_bytes()).to_string());
        let content_length = response.content_length();
        let session_id = response
            .headers()
            .get(HEADER_SESSION_ID)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        // Spec requires 202 Accepted for these, but some servers return an empty 200.
        // Treat empty success responses as equivalent to Accepted.
        if status.is_success()
            && content_length == Some(0)
            && matches!(
                message,
                ClientJsonRpcMessage::Notification(_)
                    | ClientJsonRpcMessage::Response(_)
                    | ClientJsonRpcMessage::Error(_)
            )
        {
            return Ok(StreamableHttpPostResponse::Accepted);
        }
        // Non-success responses may carry valid JSON-RPC error payloads that
        // should be surfaced as McpError rather than lost in TransportSend.
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<failed to read response body>".to_owned());
            if content_type
                .as_deref()
                .is_some_and(|ct| ct.as_bytes().starts_with(JSON_MIME_TYPE.as_bytes()))
            {
                match parse_json_rpc_error(&body) {
                    Some(message) => {
                        return Ok(StreamableHttpPostResponse::Json(message, session_id));
                    }
                    None => tracing::warn!(
                        "HTTP {status}: could not parse JSON body as a JSON-RPC error"
                    ),
                }
            }
            return Err(StreamableHttpError::UnexpectedServerResponse(Cow::Owned(
                format!("HTTP {status}: {body}"),
            )));
        }
        match content_type.as_deref() {
            Some(ct) if ct.as_bytes().starts_with(EVENT_STREAM_MIME_TYPE.as_bytes()) => {
                let event_stream = SseStream::from_bytes_stream(response.bytes_stream()).boxed();
                Ok(StreamableHttpPostResponse::Sse(event_stream, session_id))
            }
            Some(ct) if ct.as_bytes().starts_with(JSON_MIME_TYPE.as_bytes()) => {
                // Try to parse as a valid JSON-RPC message. If the body is
                // malformed (e.g. a 200 response to a notification that lacks
                // an `id` field), treat it as accepted rather than failing.
                match response.json::<ServerJsonRpcMessage>().await {
                    Ok(message) => Ok(StreamableHttpPostResponse::Json(message, session_id)),
                    Err(e) => {
                        tracing::warn!(
                            "could not parse JSON response as ServerJsonRpcMessage, treating as accepted: {e}"
                        );
                        Ok(StreamableHttpPostResponse::Accepted)
                    }
                }
            }
            _ => {
                // unexpected content type
                tracing::error!("unexpected content type: {:?}", content_type);
                Err(StreamableHttpError::UnexpectedContentType(content_type))
            }
        }
    }
}

impl StreamableHttpClientTransport<reqwest::Client> {
    /// Creates a new transport using reqwest with the specified URI.
    ///
    /// This is a convenience method that creates a transport using the default
    /// reqwest client. This method is only available when the
    /// `transport-streamable-http-client-reqwest` feature is enabled.
    ///
    /// # Arguments
    ///
    /// * `uri` - The server URI to connect to
    ///
    /// # Example
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
    /// # Feature requirement
    ///
    /// This method requires the `transport-streamable-http-client-reqwest` feature.
    pub fn from_uri(uri: impl Into<Arc<str>>) -> Self {
        StreamableHttpClientTransport::with_client(
            Self::default_http_client(),
            StreamableHttpClientTransportConfig {
                uri: uri.into(),
                auth_header: None,
                ..Default::default()
            },
        )
    }

    /// Build this transport form a config
    ///
    /// # Arguments
    ///
    /// * `config` - The config to use with this transport
    pub fn from_config(config: StreamableHttpClientTransportConfig) -> Self {
        StreamableHttpClientTransport::with_client(Self::default_http_client(), config)
    }

    /// Build the default reqwest client for this transport.
    ///
    /// Disables idle connection pooling to avoid ~40 ms stalls caused by
    /// TCP Delayed ACK on Linux when the previous response body was not
    /// fully consumed before the pool attempts to reuse the connection.
    ///
    /// Automatic redirects are disabled so caller-supplied custom headers
    /// cannot be replayed to a redirect target.
    fn default_http_client() -> reqwest::Client {
        reqwest::Client::builder()
            .pool_max_idle_per_host(0)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("failed to build default reqwest client")
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::parse_json_rpc_error;
    use crate::{
        model::{ClientJsonRpcMessage, ClientRequest, JsonRpcMessage, PingRequest, RequestId},
        transport::streamable_http_client::{AuthRequiredError, InsufficientScopeError},
    };

    #[test]
    fn auth_required_error_new() {
        let err = AuthRequiredError::new("Bearer realm=\"test\"".to_string());
        assert_eq!(err.www_authenticate_header, "Bearer realm=\"test\"");
    }

    #[test]
    fn insufficient_scope_error_can_upgrade() {
        let with_scope = InsufficientScopeError::new(
            "Bearer scope=\"admin\"".to_string(),
            Some("admin".to_string()),
        );
        assert!(with_scope.can_upgrade());
        assert_eq!(with_scope.get_required_scope(), Some("admin"));

        let without_scope =
            InsufficientScopeError::new("Bearer error=\"insufficient_scope\"".to_string(), None);
        assert!(!without_scope.can_upgrade());
        assert_eq!(without_scope.get_required_scope(), None);
    }

    #[test]
    fn parse_json_rpc_error_returns_error_variant() {
        let body =
            r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32600,"message":"Invalid Request"}}"#;
        assert!(matches!(
            parse_json_rpc_error(body),
            Some(JsonRpcMessage::Error(_))
        ));
    }

    #[rstest]
    #[case::non_error_request(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#)]
    #[case::notification(
        r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":1}}"#
    )]
    #[case::plain_text("not json at all")]
    #[case::empty("")]
    #[case::truncated_json(r#"{"broken":"#)]
    fn parse_json_rpc_error_rejects_non_error_bodies(#[case] body: &str) {
        assert!(parse_json_rpc_error(body).is_none());
    }

    #[tokio::test]
    async fn default_http_client_does_not_leak_custom_headers_to_redirect_target()
    -> anyhow::Result<()> {
        use std::{collections::HashMap, net::SocketAddr, sync::Arc};

        use axum::{
            Router, extract::State, http::StatusCode, response::IntoResponse, routing::post,
        };
        use http::{HeaderMap, HeaderName, HeaderValue, header::LOCATION};
        use tokio::sync::Mutex;

        use super::StreamableHttpClientTransport;
        use crate::transport::streamable_http_client::{StreamableHttpClient, StreamableHttpError};

        const API_KEY_HEADER: &str = "x-api-key";
        const API_KEY_VALUE: &str = "secret";

        type CapturedHeader = Arc<Mutex<Option<String>>>;

        #[derive(Clone)]
        struct RedirectState {
            location: String,
            captured_header: CapturedHeader,
        }

        async fn capture_api_key_header(headers: &HeaderMap, captured_header: &CapturedHeader) {
            if let Some(value) = headers
                .get(API_KEY_HEADER)
                .and_then(|value| value.to_str().ok())
            {
                *captured_header.lock().await = Some(value.to_owned());
            }
        }

        async fn redirect_handler(
            State(state): State<RedirectState>,
            headers: HeaderMap,
        ) -> impl IntoResponse {
            capture_api_key_header(&headers, &state.captured_header).await;

            (
                StatusCode::TEMPORARY_REDIRECT,
                [(LOCATION, state.location)],
                "",
            )
        }

        async fn redirected_handler(
            State(captured_header): State<CapturedHeader>,
            headers: HeaderMap,
        ) -> impl IntoResponse {
            capture_api_key_header(&headers, &captured_header).await;

            (
                StatusCode::OK,
                [(http::header::CONTENT_TYPE, "application/json")],
                r#"{"jsonrpc":"2.0","id":1,"result":{}}"#,
            )
        }

        let redirected_header = Arc::new(Mutex::new(None));
        let redirected_listener =
            tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0))).await?;
        let redirected_addr = redirected_listener.local_addr()?;
        let redirected_server = tokio::spawn({
            let redirected_header = redirected_header.clone();
            async move {
                let app = Router::new()
                    .route("/capture", post(redirected_handler))
                    .with_state(redirected_header);
                axum::serve(redirected_listener, app).await
            }
        });

        let original_header = Arc::new(Mutex::new(None));
        let redirect_listener =
            tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0))).await?;
        let redirect_addr = redirect_listener.local_addr()?;
        let redirect_server = tokio::spawn({
            let state = RedirectState {
                location: format!("http://{redirected_addr}/capture"),
                captured_header: original_header.clone(),
            };
            async move {
                let app = Router::new()
                    .route("/mcp", post(redirect_handler))
                    .with_state(state);
                axum::serve(redirect_listener, app).await
            }
        });

        let mut custom_headers = HashMap::new();
        custom_headers.insert(
            HeaderName::from_static(API_KEY_HEADER),
            HeaderValue::from_static(API_KEY_VALUE),
        );
        let message = ClientJsonRpcMessage::request(
            ClientRequest::PingRequest(PingRequest::default()),
            RequestId::Number(1),
        );

        let client = StreamableHttpClientTransport::<reqwest::Client>::default_http_client();
        let result = client
            .post_message(
                Arc::<str>::from(format!("http://{redirect_addr}/mcp")),
                message,
                None,
                None,
                custom_headers,
            )
            .await;

        assert!(
            matches!(
                result,
                Err(StreamableHttpError::UnexpectedServerResponse(_))
            ),
            "redirect response should be returned to the transport, got {result:?}"
        );
        assert_eq!(original_header.lock().await.as_deref(), Some(API_KEY_VALUE));
        assert!(
            redirected_header.lock().await.is_none(),
            "custom headers should not be sent to redirect targets"
        );

        redirect_server.abort();
        redirected_server.abort();

        Ok(())
    }
}
