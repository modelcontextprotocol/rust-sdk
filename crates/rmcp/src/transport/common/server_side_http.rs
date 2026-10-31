#![allow(dead_code)]
use std::{convert::Infallible, fmt::Display, sync::Arc, time::Duration};

use bytes::{Buf, Bytes};
use http::Response;
use http_body::Body;
use http_body_util::{BodyExt, Empty, Full, combinators::BoxBody};
use sse_stream::{KeepAlive, Sse, SseBody};
use tokio_util::sync::{CancellationToken, DropGuard};

use super::http_header::{EVENT_STREAM_MIME_TYPE, JSON_MIME_TYPE};
use crate::model::{
    ClientJsonRpcMessage, ErrorData, JsonRpcError, RequestId, ServerJsonRpcMessage,
};

pub type SessionId = Arc<str>;

pub fn session_id() -> SessionId {
    uuid::Uuid::new_v4().to_string().into()
}

pub const DEFAULT_AUTO_PING_INTERVAL: Duration = Duration::from_secs(15);

pub(crate) type BoxResponse = Response<BoxBody<Bytes, Infallible>>;

pub(crate) fn accepted_response() -> Response<BoxBody<Bytes, Infallible>> {
    Response::builder()
        .status(http::StatusCode::ACCEPTED)
        .body(Empty::new().boxed())
        .expect("valid response")
}
pin_project_lite::pin_project! {
    struct TokioTimer {
        #[pin]
        sleep: tokio::time::Sleep,
    }
}
impl Future for TokioTimer {
    type Output = ();

    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let this = self.project();
        this.sleep.poll(cx)
    }
}
impl sse_stream::Timer for TokioTimer {
    fn from_duration(duration: Duration) -> Self {
        Self {
            sleep: tokio::time::sleep(duration),
        }
    }

    fn reset(self: std::pin::Pin<&mut Self>, when: std::time::Instant) {
        let this = self.project();
        this.sleep.reset(tokio::time::Instant::from_std(when));
    }
}

pin_project_lite::pin_project! {
    struct CancelOnDropStream<S> {
        #[pin]
        inner: S,
        _drop_guard: DropGuard,
    }
}

impl<S: futures::Stream> futures::Stream for CancelOnDropStream<S> {
    type Item = S::Item;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        self.project().inner.poll_next(cx)
    }
}

#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct ServerSseMessage {
    /// The event ID for this message. When set, clients can use this ID
    /// with the `Last-Event-ID` header to resume the stream from this point.
    pub event_id: Option<String>,
    /// The JSON-RPC message content. Set to `None` for priming events.
    /// See [SEP-1699](https://github.com/modelcontextprotocol/modelcontextprotocol/issues/1699)
    pub message: Option<Arc<ServerJsonRpcMessage>>,
    /// The retry interval hint for clients. Clients should wait this duration
    /// before attempting to reconnect. This maps to the SSE `retry:` field.
    pub retry: Option<Duration>,
}

impl ServerSseMessage {
    /// Create a message carrying a JSON-RPC response/notification with an event ID.
    pub fn new(event_id: impl Into<String>, message: ServerJsonRpcMessage) -> Self {
        Self {
            event_id: Some(event_id.into()),
            message: Some(Arc::new(message)),
            retry: None,
        }
    }

    /// Wrap a JSON-RPC message without an event ID or retry hint.
    pub fn from_message(message: ServerJsonRpcMessage) -> Self {
        Self {
            event_id: None,
            message: Some(Arc::new(message)),
            retry: None,
        }
    }

    /// Create a priming event that tells the client to reconnect after `retry`
    /// if the connection drops.
    /// See [SEP-1699](https://github.com/modelcontextprotocol/modelcontextprotocol/issues/1699).
    pub fn priming(event_id: impl Into<String>, retry: Duration) -> Self {
        Self {
            event_id: Some(event_id.into()),
            message: None,
            retry: Some(retry),
        }
    }

    /// Create a retry hint without changing the client's last event ID.
    pub fn retry(retry: Duration) -> Self {
        Self {
            event_id: None,
            message: None,
            retry: Some(retry),
        }
    }
}

pub(crate) fn sse_stream_response(
    stream: impl futures::Stream<Item = ServerSseMessage> + Send + Sync + 'static,
    keep_alive: Option<Duration>,
    ct: CancellationToken,
) -> Response<BoxBody<Bytes, Infallible>> {
    use futures::StreamExt;
    let cancelled = ct.clone();
    let stream = stream
        .map(|message| {
            let mut sse = if let Some(ref msg) = message.message {
                let data = serde_json::to_string(msg.as_ref()).expect("valid message");
                Sse::default().data(data)
            } else {
                // Priming event: empty data per SEP-1699 (just "data:\n")
                Sse::default().data("")
            };

            sse.id = message.event_id;

            if let Some(retry) = message.retry {
                sse.retry = Some(retry.as_millis() as u64);
            }

            Result::<Sse, Infallible>::Ok(sse)
        })
        .take_until(async move { cancelled.cancelled().await });
    let stream = CancelOnDropStream {
        inner: stream,
        _drop_guard: ct.drop_guard(),
    };
    let stream = SseBody::new(stream);

    let stream = match keep_alive {
        Some(duration) => stream
            .with_keep_alive::<TokioTimer>(KeepAlive::new().interval(duration))
            .boxed(),
        None => stream.boxed(),
    };

    Response::builder()
        .status(http::StatusCode::OK)
        .header(http::header::CONTENT_TYPE, EVENT_STREAM_MIME_TYPE)
        .header(http::header::CACHE_CONTROL, "no-cache")
        .header("X-Accel-Buffering", "no")
        .body(stream)
        .expect("valid response")
}

pub(crate) const fn internal_error_response<E: Display>(
    context: &str,
) -> impl FnOnce(E) -> Response<BoxBody<Bytes, Infallible>> {
    move |error| {
        tracing::error!("Internal server error when {context}: {error}");
        Response::builder()
            .status(http::StatusCode::INTERNAL_SERVER_ERROR)
            .body(
                Full::new(Bytes::from(format!(
                    "Encounter an error when {context}: {error}"
                )))
                .boxed(),
            )
            .expect("valid response")
    }
}

/// MCP spec: servers that require a session ID SHOULD respond to requests
/// without one (other than initialization) with 400 Bad Request.
pub(crate) fn session_id_required_response() -> Response<BoxBody<Bytes, Infallible>> {
    Response::builder()
        .status(http::StatusCode::BAD_REQUEST)
        .body(Full::new(Bytes::from("Bad Request: Session ID is required")).boxed())
        .expect("valid response")
}

pub(crate) async fn expect_json<B>(
    body: B,
    max_bytes: usize,
) -> Result<ClientJsonRpcMessage, Response<BoxBody<Bytes, Infallible>>>
where
    B: Body + Send + 'static,
    B::Error: Display,
{
    let mut collected = bytes::BytesMut::new();
    let mut body = std::pin::pin!(body);
    loop {
        let frame = futures::future::poll_fn(|cx| body.as_mut().poll_frame(cx)).await;
        match frame {
            None => break,
            Some(Ok(frame)) => {
                let Ok(mut data) = frame.into_data() else {
                    continue;
                };
                if data.remaining() > max_bytes.saturating_sub(collected.len()) {
                    let response = Response::builder()
                        .status(http::StatusCode::PAYLOAD_TOO_LARGE)
                        .body(
                            Full::new(Bytes::from(format!(
                                "Payload Too Large: request body exceeds {max_bytes} bytes"
                            )))
                            .boxed(),
                        )
                        .expect("valid response");
                    return Err(response);
                }
                while data.has_remaining() {
                    let chunk = data.chunk();
                    let chunk_len = chunk.len();
                    collected.extend_from_slice(chunk);
                    data.advance(chunk_len);
                }
            }
            Some(Err(e)) => {
                let response = Response::builder()
                    .status(http::StatusCode::INTERNAL_SERVER_ERROR)
                    .body(
                        Full::new(Bytes::from(format!("Failed to read request body: {e}"))).boxed(),
                    )
                    .expect("valid response");
                return Err(response);
            }
        }
    }

    match serde_json::from_slice::<ClientJsonRpcMessage>(&collected) {
        Ok(message) => Ok(message),
        Err(e) => Err(malformed_body_response(&collected, &e)),
    }
}

/// The Content-Type was already accepted, so a body that isn't a JSON-RPC
/// message is a bad request (not 415). The 400 carries a JSON-RPC error:
/// Parse error for invalid JSON, Invalid Request for valid JSON of the wrong
/// shape. The id is echoed when it can be read, and omitted otherwise.
fn malformed_body_response(
    body: &[u8],
    e: &serde_json::Error,
) -> Response<BoxBody<Bytes, Infallible>> {
    // A typed deserialize reports a shape error before it checks for trailing
    // bytes (`null trailing`), so check the whole body is JSON on its own.
    let (id, error) = match serde_json::from_slice::<serde_json::Value>(body) {
        Err(syntax) => (
            None,
            ErrorData::parse_error(format!("Parse error: {syntax}"), None),
        ),
        Ok(value) => (
            value
                .get("id")
                .and_then(|id| serde_json::from_value::<RequestId>(id.clone()).ok()),
            ErrorData::invalid_request(format!("Invalid request: {e}"), None),
        ),
    };
    let body = serde_json::to_vec(&JsonRpcError::new(id, error)).expect("serialize JsonRpcError");
    Response::builder()
        .status(http::StatusCode::BAD_REQUEST)
        .header(http::header::CONTENT_TYPE, JSON_MIME_TYPE)
        .body(Full::new(Bytes::from(body)).boxed())
        .expect("valid response")
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;

    use futures::stream;
    use http_body::Frame;
    use http_body_util::StreamBody;

    use super::*;
    use crate::model::{EmptyResult, JsonRpcResponse, JsonRpcVersion2_0, RequestId, ServerResult};

    const INITIALIZE_REQUEST: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"0.1"}}}"#;

    fn dummy_message() -> ServerJsonRpcMessage {
        ServerJsonRpcMessage::Response(JsonRpcResponse {
            jsonrpc: JsonRpcVersion2_0,
            id: RequestId::Number(1),
            result: ServerResult::EmptyResult(EmptyResult {}),
        })
    }

    #[test]
    fn default_has_all_none() {
        let msg = ServerSseMessage::default();
        assert!(msg.event_id.is_none());
        assert!(msg.message.is_none());
        assert!(msg.retry.is_none());
    }

    #[test]
    fn new_sets_event_id_and_message() {
        let msg = ServerSseMessage::new("42", dummy_message());
        assert_eq!(msg.event_id.as_deref(), Some("42"));
        assert!(msg.message.is_some());
        assert!(msg.retry.is_none());
    }

    #[test]
    fn from_message_has_no_event_id() {
        let msg = ServerSseMessage::from_message(dummy_message());
        assert!(msg.event_id.is_none());
        assert!(msg.message.is_some());
        assert!(msg.retry.is_none());
    }

    #[test]
    fn priming_sets_event_id_and_retry() {
        let msg = ServerSseMessage::priming("0", Duration::from_secs(5));
        assert_eq!(msg.event_id.as_deref(), Some("0"));
        assert!(msg.message.is_none());
        assert_eq!(msg.retry, Some(Duration::from_secs(5)));
    }

    #[tokio::test]
    async fn expect_json_accepts_body_under_limit() {
        let body = Full::new(Bytes::from_static(INITIALIZE_REQUEST.as_bytes()));
        let result = expect_json(body, 4 * 1024 * 1024).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn expect_json_accepts_non_contiguous_body_data() {
        let split = INITIALIZE_REQUEST.len() / 2;
        let data = Bytes::copy_from_slice(&INITIALIZE_REQUEST.as_bytes()[..split]).chain(
            Bytes::copy_from_slice(&INITIALIZE_REQUEST.as_bytes()[split..]),
        );
        let result = expect_json(Full::new(data), 4 * 1024 * 1024).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn expect_json_accepts_multi_frame_body() {
        let split = INITIALIZE_REQUEST.len() / 2;
        let body = StreamBody::new(stream::iter([
            Ok::<_, Infallible>(Frame::data(Bytes::copy_from_slice(
                &INITIALIZE_REQUEST.as_bytes()[..split],
            ))),
            Ok(Frame::data(Bytes::copy_from_slice(
                &INITIALIZE_REQUEST.as_bytes()[split..],
            ))),
        ]));
        let result = expect_json(body, 4 * 1024 * 1024).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn expect_json_rejects_oversized_body() {
        let big_body = Full::new(Bytes::from(vec![b'x'; 128]));
        let result = expect_json(big_body, 64).await;
        let response = result.unwrap_err();
        assert_eq!(response.status(), http::StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn expect_json_rejects_oversized_multi_frame_body() {
        let body = StreamBody::new(stream::iter([
            Ok::<_, Infallible>(Frame::data(Bytes::from_static(b"12345678"))),
            Ok(Frame::data(Bytes::from_static(b"9"))),
        ]));
        let response = expect_json(body, 8).await.unwrap_err();
        assert_eq!(response.status(), http::StatusCode::PAYLOAD_TOO_LARGE);
    }

    async fn rejected_body(body: &'static str) -> (http::StatusCode, serde_json::Value) {
        let response = expect_json(Full::new(Bytes::from(body)), 4 * 1024 * 1024)
            .await
            .unwrap_err();
        let status = response.status();
        assert_eq!(
            response.headers().get(http::header::CONTENT_TYPE).unwrap(),
            JSON_MIME_TYPE
        );
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    #[rstest::rstest]
    #[case::not_json("not valid json")]
    #[case::truncated(r#"{"jsonrpc":"2.0","id":1,"#)]
    #[case::empty("")]
    #[case::trailing_after_null("null trailing")]
    #[case::trailing_after_array("[] {")]
    #[tokio::test]
    async fn expect_json_answers_invalid_json_with_400_parse_error(#[case] body: &'static str) {
        let (status, error) = rejected_body(body).await;
        assert_eq!(status, http::StatusCode::BAD_REQUEST);
        assert_eq!(error["jsonrpc"], "2.0");
        assert_eq!(error["error"]["code"], -32700);
        assert!(error.get("id").is_none(), "no id can be read: {error}");
    }

    #[rstest::rstest]
    #[case::null("null")]
    #[case::array("[]")]
    #[case::null_id(r#"{"jsonrpc":"2.0","id":null,"method":"ping"}"#)]
    #[case::object_id(r#"{"jsonrpc":"2.0","id":{},"method":"ping"}"#)]
    #[tokio::test]
    async fn expect_json_answers_wrong_shape_with_400_invalid_request(#[case] body: &'static str) {
        let (status, error) = rejected_body(body).await;
        assert_eq!(status, http::StatusCode::BAD_REQUEST);
        assert_eq!(error["error"]["code"], -32600);
        assert!(error.get("id").is_none(), "no id can be read: {error}");
    }

    #[rstest::rstest]
    #[case::number_id(r#"{"jsonrpc":"1.0","id":1,"method":"ping"}"#, serde_json::json!(1))]
    #[case::string_id(r#"{"jsonrpc":"1.0","id":"a","method":"ping"}"#, serde_json::json!("a"))]
    #[case::no_method(r#"{"jsonrpc":"2.0","id":7}"#, serde_json::json!(7))]
    #[tokio::test]
    async fn expect_json_echoes_readable_id_in_invalid_request(
        #[case] body: &'static str,
        #[case] id: serde_json::Value,
    ) {
        let (status, error) = rejected_body(body).await;
        assert_eq!(status, http::StatusCode::BAD_REQUEST);
        assert_eq!(error["error"]["code"], -32600);
        assert_eq!(error["id"], id);
    }
}
