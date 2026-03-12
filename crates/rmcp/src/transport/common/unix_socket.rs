use std::{borrow::Cow, collections::HashMap, sync::Arc};

use bytes::Bytes;
use futures::{StreamExt, stream::BoxStream};
use http::{HeaderName, HeaderValue, Method, Request, StatusCode, header::WWW_AUTHENTICATE};
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper_util::rt::TokioIo;
use sse_stream::{Sse, SseStream};
use tokio::net::UnixStream;

use crate::{
    model::{ClientJsonRpcMessage, ServerJsonRpcMessage},
    transport::{
        common::http_header::{
            EVENT_STREAM_MIME_TYPE, HEADER_LAST_EVENT_ID, HEADER_SESSION_ID, JSON_MIME_TYPE,
            extract_scope_from_header, validate_custom_header,
        },
        streamable_http_client::*,
    },
};

#[derive(Debug, thiserror::Error)]
pub enum UnixSocketError {
    #[error("hyper error: {0}")]
    Hyper(#[from] hyper::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("HTTP error: {0}")]
    Http(#[from] http::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

impl From<UnixSocketError> for StreamableHttpError<UnixSocketError> {
    fn from(e: UnixSocketError) -> Self {
        StreamableHttpError::Client(e)
    }
}

/// HTTP client that routes requests through a Unix domain socket.
///
/// Implements [`StreamableHttpClient`] using `hyper` over `tokio::net::UnixStream`,
/// enabling MCP hosts in Kubernetes environments to connect through Envoy sidecars
/// or other Unix socket-based proxies.
///
/// Each request opens a new Unix socket connection (no connection pooling).
/// This is appropriate when connecting through a sidecar proxy that manages
/// its own upstream connection pool.
///
/// # Example
///
/// ```rust,no_run
/// use rmcp::transport::{StreamableHttpClientTransport, UnixSocketHttpClient};
/// use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
///
/// let client = UnixSocketHttpClient::new("/var/run/envoy.sock", "http://mcp-server.internal/mcp");
/// let config = StreamableHttpClientTransportConfig::with_uri("http://mcp-server.internal/mcp");
/// let transport = StreamableHttpClientTransport::with_client(client, config);
/// ```
#[derive(Clone, Debug)]
pub struct UnixSocketHttpClient {
    socket_path: Arc<str>,
    host_header: HeaderValue,
}

impl UnixSocketHttpClient {
    /// Creates a new Unix socket HTTP client.
    ///
    /// # Arguments
    ///
    /// * `socket_path` - Path to the Unix domain socket. Use `@name` syntax for Linux
    ///   abstract sockets (e.g., `@egress.sock` becomes `\0egress.sock`).
    /// * `uri` - The MCP server URI. The authority (host:port) is extracted for the
    ///   HTTP `Host` header, since hyper does not auto-set it for Unix socket connections.
    ///
    /// # Panics
    ///
    /// Panics if `socket_path` is empty or is `@` with no name (empty abstract socket).
    pub fn new(socket_path: &str, uri: &str) -> Self {
        assert!(
            !socket_path.is_empty() && socket_path != "@",
            "socket_path must not be empty or a bare '@' (empty abstract socket name)"
        );

        let host_header = uri
            .parse::<http::Uri>()
            .ok()
            .and_then(|u| u.authority().cloned())
            .and_then(|a| HeaderValue::from_str(a.as_str()).ok())
            .unwrap_or_else(|| HeaderValue::from_static("localhost"));

        Self {
            socket_path: resolve_socket_path(socket_path).into(),
            host_header,
        }
    }
}

/// Converts the `@`-prefixed abstract socket notation to the null-byte prefix
/// expected by the Linux kernel. Filesystem socket paths are returned unchanged.
fn resolve_socket_path(raw: &str) -> String {
    if let Some(name) = raw.strip_prefix('@') {
        format!("\0{name}")
    } else {
        raw.to_string()
    }
}

async fn connect_unix(socket_path: &str) -> Result<UnixStream, std::io::Error> {
    #[cfg(target_os = "linux")]
    if let Some(abstract_name) = socket_path.strip_prefix('\0') {
        let abstract_name = abstract_name.to_string();
        let std_stream = tokio::task::spawn_blocking(move || {
            use std::os::linux::net::SocketAddrExt;
            let addr = std::os::unix::net::SocketAddr::from_abstract_name(&abstract_name)?;
            let stream = std::os::unix::net::UnixStream::connect_addr(&addr)?;
            stream.set_nonblocking(true)?;
            Ok::<_, std::io::Error>(stream)
        })
        .await
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))??;
        return UnixStream::from_std(std_stream);
    }

    UnixStream::connect(socket_path).await
}

/// Opens a new Unix socket connection and sends the HTTP request.
/// One connection per request — the sidecar proxy handles connection pooling.
async fn send_http_request(
    socket_path: &str,
    request: Request<Full<Bytes>>,
) -> Result<http::Response<Incoming>, UnixSocketError> {
    let stream = connect_unix(socket_path).await?;
    let io = TokioIo::new(stream);
    let (mut sender, conn) = hyper::client::conn::http1::handshake(io).await?;

    tokio::spawn(async move {
        if let Err(e) = conn.await {
            tracing::warn!("unix socket HTTP/1.1 connection error: {e}");
        }
    });

    Ok(sender.send_request(request).await?)
}

/// Applies custom headers to a request builder, rejecting reserved headers.
fn apply_custom_headers(
    mut builder: http::request::Builder,
    custom_headers: HashMap<HeaderName, HeaderValue>,
) -> Result<http::request::Builder, StreamableHttpError<UnixSocketError>> {
    for (name, value) in custom_headers {
        validate_custom_header(&name).map_err(StreamableHttpError::ReservedHeaderConflict)?;
        builder = builder.header(name, value);
    }
    Ok(builder)
}

impl StreamableHttpClient for UnixSocketHttpClient {
    type Error = UnixSocketError;

    async fn post_message(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_token: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>> {
        let json_body = serde_json::to_string(&message)
            .map_err(|e| StreamableHttpError::Client(UnixSocketError::Json(e)))?;

        let mut builder = Request::builder()
            .method(Method::POST)
            .uri(uri.as_ref())
            .header(http::header::HOST, self.host_header.clone())
            .header(http::header::CONTENT_TYPE, JSON_MIME_TYPE)
            .header(
                http::header::ACCEPT,
                format!("{EVENT_STREAM_MIME_TYPE}, {JSON_MIME_TYPE}"),
            );

        if let Some(auth) = auth_token {
            builder = builder.header(http::header::AUTHORIZATION, format!("Bearer {auth}"));
        }

        builder = apply_custom_headers(builder, custom_headers)?;

        let session_was_attached = session_id.is_some();
        if let Some(sid) = session_id {
            builder = builder.header(HEADER_SESSION_ID, sid.as_ref());
        }

        let request = builder
            .body(Full::new(Bytes::from(json_body)))
            .map_err(|e| StreamableHttpError::Client(UnixSocketError::Http(e)))?;

        let response = send_http_request(&self.socket_path, request)
            .await
            .map_err(StreamableHttpError::Client)?;

        let status = response.status();

        if status == StatusCode::UNAUTHORIZED {
            if let Some(header) = response.headers().get(WWW_AUTHENTICATE) {
                let www_authenticate_header = header
                    .to_str()
                    .map_err(|_| {
                        StreamableHttpError::UnexpectedServerResponse(Cow::from(
                            "invalid www-authenticate header value",
                        ))
                    })?
                    .to_string();
                return Err(StreamableHttpError::AuthRequired(AuthRequiredError {
                    www_authenticate_header,
                }));
            }
        }

        if status == StatusCode::FORBIDDEN {
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

        if matches!(status, StatusCode::ACCEPTED | StatusCode::NO_CONTENT) {
            return Ok(StreamableHttpPostResponse::Accepted);
        }

        if status == StatusCode::NOT_FOUND && session_was_attached {
            return Err(StreamableHttpError::SessionExpired);
        }

        if !status.is_success() {
            let body = response
                .into_body()
                .collect()
                .await
                .map(|c| String::from_utf8_lossy(&c.to_bytes()).into_owned())
                .unwrap_or_else(|_| "<failed to read response body>".to_owned());
            return Err(StreamableHttpError::UnexpectedServerResponse(Cow::Owned(
                format!("HTTP {status}: {body}"),
            )));
        }

        let content_type = response.headers().get(http::header::CONTENT_TYPE).cloned();
        let session_id = response
            .headers()
            .get(HEADER_SESSION_ID)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        match content_type {
            Some(ref ct) if ct.as_bytes().starts_with(EVENT_STREAM_MIME_TYPE.as_bytes()) => {
                let sse_stream = SseStream::new(response.into_body()).boxed();
                Ok(StreamableHttpPostResponse::Sse(sse_stream, session_id))
            }
            Some(ref ct) if ct.as_bytes().starts_with(JSON_MIME_TYPE.as_bytes()) => {
                let body = response
                    .into_body()
                    .collect()
                    .await
                    .map_err(|e| StreamableHttpError::Client(UnixSocketError::Hyper(e)))?
                    .to_bytes();
                match serde_json::from_slice::<ServerJsonRpcMessage>(&body) {
                    Ok(message) => Ok(StreamableHttpPostResponse::Json(message, session_id)),
                    Err(e) => {
                        tracing::warn!(
                            "could not parse JSON response as ServerJsonRpcMessage, treating as accepted: {e}"
                        );
                        Ok(StreamableHttpPostResponse::Accepted)
                    }
                }
            }
            _ => Err(StreamableHttpError::UnexpectedContentType(
                content_type.map(|ct| String::from_utf8_lossy(ct.as_bytes()).into_owned()),
            )),
        }
    }

    async fn delete_session(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        auth_token: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<(), StreamableHttpError<Self::Error>> {
        let mut builder = Request::builder()
            .method(Method::DELETE)
            .uri(uri.as_ref())
            .header(http::header::HOST, self.host_header.clone())
            .header(HEADER_SESSION_ID, session_id.as_ref());

        if let Some(auth) = auth_token {
            builder = builder.header(http::header::AUTHORIZATION, format!("Bearer {auth}"));
        }

        builder = apply_custom_headers(builder, custom_headers)?;

        let request = builder
            .body(Full::new(Bytes::new()))
            .map_err(|e| StreamableHttpError::Client(UnixSocketError::Http(e)))?;

        let response = send_http_request(&self.socket_path, request)
            .await
            .map_err(StreamableHttpError::Client)?;

        if response.status() == StatusCode::METHOD_NOT_ALLOWED {
            tracing::debug!("this server doesn't support deleting session");
            return Ok(());
        }

        if !response.status().is_success() {
            return Err(StreamableHttpError::UnexpectedServerResponse(Cow::Owned(
                format!("delete_session returned {}", response.status()),
            )));
        }

        Ok(())
    }

    async fn get_stream(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        last_event_id: Option<String>,
        auth_token: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<BoxStream<'static, Result<Sse, sse_stream::Error>>, StreamableHttpError<Self::Error>>
    {
        let mut builder = Request::builder()
            .method(Method::GET)
            .uri(uri.as_ref())
            .header(http::header::HOST, self.host_header.clone())
            .header(
                http::header::ACCEPT,
                format!("{EVENT_STREAM_MIME_TYPE}, {JSON_MIME_TYPE}"),
            )
            .header(HEADER_SESSION_ID, session_id.as_ref());

        if let Some(last_id) = last_event_id {
            builder = builder.header(HEADER_LAST_EVENT_ID, last_id);
        }

        if let Some(auth) = auth_token {
            builder = builder.header(http::header::AUTHORIZATION, format!("Bearer {auth}"));
        }

        builder = apply_custom_headers(builder, custom_headers)?;

        let request = builder
            .body(Full::new(Bytes::new()))
            .map_err(|e| StreamableHttpError::Client(UnixSocketError::Http(e)))?;

        let response = send_http_request(&self.socket_path, request)
            .await
            .map_err(StreamableHttpError::Client)?;

        if response.status() == StatusCode::METHOD_NOT_ALLOWED {
            return Err(StreamableHttpError::ServerDoesNotSupportSse);
        }

        if response.status() == StatusCode::UNAUTHORIZED {
            if let Some(header) = response.headers().get(WWW_AUTHENTICATE) {
                let www_authenticate_header = header
                    .to_str()
                    .map_err(|_| {
                        StreamableHttpError::UnexpectedServerResponse(Cow::from(
                            "invalid www-authenticate header value",
                        ))
                    })?
                    .to_string();
                return Err(StreamableHttpError::AuthRequired(AuthRequiredError {
                    www_authenticate_header,
                }));
            }
        }

        if response.status() == StatusCode::FORBIDDEN {
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

        if !response.status().is_success() {
            return Err(StreamableHttpError::UnexpectedServerResponse(Cow::Owned(
                format!("get_stream returned {}", response.status()),
            )));
        }

        match response.headers().get(http::header::CONTENT_TYPE) {
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

        Ok(SseStream::new(response.into_body()).boxed())
    }
}

impl StreamableHttpClientTransport<UnixSocketHttpClient> {
    /// Creates a new transport connecting through a Unix domain socket.
    ///
    /// # Arguments
    ///
    /// * `socket_path` - Path to the Unix domain socket. Use `@name` for Linux abstract sockets.
    /// * `uri` - The MCP server URI (used for HTTP Host header and request path).
    pub fn from_unix_socket(socket_path: &str, uri: impl Into<Arc<str>>) -> Self {
        let uri: Arc<str> = uri.into();
        let client = UnixSocketHttpClient::new(socket_path, &uri);
        let config = StreamableHttpClientTransportConfig {
            uri,
            ..Default::default()
        };
        StreamableHttpClientTransport::with_client(client, config)
    }

    /// Creates a new transport connecting through a Unix domain socket with custom config.
    ///
    /// # Arguments
    ///
    /// * `socket_path` - Path to the Unix domain socket. Use `@name` for Linux abstract sockets.
    /// * `config` - Transport configuration (URI, retry policy, custom headers, etc.).
    pub fn from_unix_socket_with_config(
        socket_path: &str,
        config: StreamableHttpClientTransportConfig,
    ) -> Self {
        let client = UnixSocketHttpClient::new(socket_path, &config.uri);
        StreamableHttpClientTransport::with_client(client, config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_abstract_socket() {
        assert_eq!(resolve_socket_path("@egress.sock"), "\0egress.sock");
    }

    #[test]
    fn resolve_filesystem_socket() {
        assert_eq!(
            resolve_socket_path("/var/run/envoy.sock"),
            "/var/run/envoy.sock"
        );
    }

    #[test]
    fn resolve_empty_abstract() {
        assert_eq!(resolve_socket_path("@"), "\0");
    }

    #[test]
    #[should_panic(expected = "socket_path must not be empty")]
    fn rejects_bare_at_symbol() {
        UnixSocketHttpClient::new("@", "http://localhost/mcp");
    }

    #[test]
    #[should_panic(expected = "socket_path must not be empty")]
    fn rejects_empty_path() {
        UnixSocketHttpClient::new("", "http://localhost/mcp");
    }

    #[test]
    fn host_header_auto_derived() {
        let client =
            UnixSocketHttpClient::new("/var/run/envoy.sock", "http://mcp-server.internal/mcp");
        assert_eq!(client.host_header, "mcp-server.internal");
    }

    #[test]
    fn host_header_with_port() {
        let client =
            UnixSocketHttpClient::new("/var/run/envoy.sock", "http://mcp-server.internal:8080/mcp");
        assert_eq!(client.host_header, "mcp-server.internal:8080");
    }

    #[test]
    fn host_header_fallback_on_path_only_uri() {
        let client = UnixSocketHttpClient::new("/var/run/envoy.sock", "/mcp");
        assert_eq!(client.host_header, "localhost");
    }

    #[test]
    fn reserved_header_rejected() {
        let mut headers = HashMap::new();
        headers.insert(
            HeaderName::from_static("accept"),
            HeaderValue::from_static("text/plain"),
        );
        let builder = Request::builder();
        let result = apply_custom_headers(builder, headers);
        assert!(matches!(
            result,
            Err(StreamableHttpError::ReservedHeaderConflict(_))
        ));
    }

    #[test]
    fn mcp_protocol_version_allowed_through() {
        let mut headers = HashMap::new();
        headers.insert(
            HeaderName::from_static("mcp-protocol-version"),
            HeaderValue::from_static("2025-03-26"),
        );
        let builder = Request::builder().uri("http://localhost/mcp").method("GET");
        let result = apply_custom_headers(builder, headers);
        assert!(result.is_ok());
    }
}
