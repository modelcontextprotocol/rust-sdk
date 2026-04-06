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
        let event_stream = SseStream::from_byte_stream(response.bytes_stream()).boxed();
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
        let session_id = response
            .headers()
            .get(HEADER_SESSION_ID)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
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
                let event_stream = SseStream::from_byte_stream(response.bytes_stream()).boxed();
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
    /// Disables idle connection pooling because POST SSE response bodies
    /// cannot be fully consumed before the next request starts, causing
    /// reqwest to stall on pool checkout (~40 ms).  The long-lived GET
    /// SSE stream already holds its own connection, so there is no
    /// benefit to pooling POST connections with HTTP/1.1.
    fn default_http_client() -> reqwest::Client {
        reqwest::Client::builder()
            .pool_max_idle_per_host(0)
            .build()
            .expect("failed to build default reqwest client")
    }
}

#[cfg(test)]
mod tests {
    use super::parse_json_rpc_error;
    use crate::{model::JsonRpcMessage, transport::streamable_http_client::InsufficientScopeError};

    #[test]
    fn insufficient_scope_error_can_upgrade() {
        let with_scope = InsufficientScopeError {
            www_authenticate_header: "Bearer scope=\"admin\"".to_string(),
            required_scope: Some("admin".to_string()),
        };
        assert!(with_scope.can_upgrade());
        assert_eq!(with_scope.get_required_scope(), Some("admin"));

        let without_scope = InsufficientScopeError {
            www_authenticate_header: "Bearer error=\"insufficient_scope\"".to_string(),
            required_scope: None,
        };
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

    #[test]
    fn parse_json_rpc_error_rejects_non_error_request() {
        // A valid JSON-RPC request (method + id) must not be accepted as an error.
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
        assert!(parse_json_rpc_error(body).is_none());
    }

    #[test]
    fn parse_json_rpc_error_rejects_notification() {
        // A notification (method, no id) must not be accepted as an error.
        let body =
            r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":1}}"#;
        assert!(parse_json_rpc_error(body).is_none());
    }

    #[test]
    fn parse_json_rpc_error_rejects_malformed_json() {
        assert!(parse_json_rpc_error("not json at all").is_none());
        assert!(parse_json_rpc_error("").is_none());
        assert!(parse_json_rpc_error(r#"{"broken":"#).is_none());
    }
}
