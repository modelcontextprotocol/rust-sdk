#![cfg(all(
    feature = "transport-streamable-http-client",
    feature = "transport-streamable-http-client-reqwest",
    not(feature = "local")
))]
//! The reqwest backend reads non-SSE POST bodies only up to a fixed size: JSON
//! success bodies up to 16 MiB, error bodies up to 64 KiB. An oversized body
//! fails closed: no prefix is parsed and a notification is not reported as
//! accepted. The client must also stop reading a body that is still open
//! once the limit is exceeded.
//!
//! A raw TCP fixture controls the framing exactly, including `Content-Length`
//! values that do not match the bytes sent, chunked bodies without a final
//! chunk, and connections kept open after the body.

use std::{collections::HashMap, fmt::Debug, io, sync::Arc, time::Duration};

use rmcp::{
    model::{
        ClientJsonRpcMessage, ClientNotification, ClientRequest, DiscoverRequest,
        DiscoverRequestParams, InitializedNotification, JsonRpcMessage, PingRequest, RequestId,
    },
    transport::streamable_http_client::{
        StreamableHttpClient, StreamableHttpError, StreamableHttpPostResponse,
    },
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    task::{JoinHandle, JoinSet},
};

/// Must match the private limits in the reqwest backend.
const MAX_JSON_BODY: usize = 16 * 1024 * 1024;
const MAX_ERROR_BODY: usize = 64 * 1024;
/// Far above any limit; never sent, only declared.
const HUGE_DECLARED: u64 = 1 << 40;
const JSON_OVER: u64 = MAX_JSON_BODY as u64 + 1;
const ERROR_OVER: u64 = MAX_ERROR_BODY as u64 + 1;
/// Generous; a bounded read finishes in well under a second.
const DEADLINE: Duration = Duration::from_secs(10);
const WRITE_CHUNK: usize = 64 * 1024;

const RESULT_JSON: &str = r#"{"jsonrpc":"2.0","id":1,"result":{}}"#;
const ERROR_JSON: &str =
    r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32603,"message":"bounded-error"}}"#;
const TEXT: &str = "private-error-body";
const SERVER_ERROR: i32 = -32603;
const INVALID_REQUEST: i32 = -32600;
const CHALLENGE: &str = "Bearer realm=\"test\"";

type PostResult = Result<StreamableHttpPostResponse, StreamableHttpError<reqwest::Error>>;

#[derive(Clone, Copy, Debug)]
enum Framing {
    /// `Content-Length` equal to the bytes sent.
    Length,
    /// `Content-Length` with this value, whatever is sent.
    Declared(u64),
    /// Chunked; the final chunk is sent only with [`End::Close`].
    Chunked,
}

#[derive(Clone, Copy, Debug)]
enum End {
    /// Finish the framing and close the connection.
    Close,
    /// Keep the connection open without finishing the body.
    Hold,
}

#[derive(Clone, Copy, Debug)]
enum Payload {
    /// Valid JSON-RPC response, padded with trailing whitespace.
    Result,
    /// Valid JSON-RPC error, padded with trailing whitespace.
    Error,
    /// Plain text.
    Text,
}

impl Payload {
    /// `len` bytes: a valid document followed by padding, so any prefix that
    /// covers the document still parses. A length of 0 sends nothing.
    fn bytes(self, len: usize) -> Arc<[u8]> {
        if len == 0 {
            return Arc::from(Vec::new());
        }
        let (prefix, fill) = match self {
            Payload::Result => (RESULT_JSON, b' '),
            Payload::Error => (ERROR_JSON, b' '),
            Payload::Text => (TEXT, b'x'),
        };
        assert!(prefix.len() <= len);
        let mut body = Vec::with_capacity(len);
        body.extend_from_slice(prefix.as_bytes());
        body.resize(len, fill);
        body.into()
    }
}

#[derive(Clone, Copy, Debug)]
enum Message {
    Request,
    Notification,
    Discover,
}

impl Message {
    fn build(self) -> ClientJsonRpcMessage {
        match self {
            Message::Request => ClientJsonRpcMessage::request(
                ClientRequest::PingRequest(PingRequest::default()),
                RequestId::Number(1),
            ),
            Message::Notification => ClientJsonRpcMessage::notification(
                ClientNotification::InitializedNotification(InitializedNotification::default()),
            ),
            Message::Discover => ClientJsonRpcMessage::request(
                ClientRequest::DiscoverRequest(DiscoverRequest::new(DiscoverRequestParams {})),
                RequestId::Number(1),
            ),
        }
    }
}

struct Reply {
    status: u16,
    content_type: &'static str,
    challenge: Option<&'static str>,
    body: Arc<[u8]>,
    framing: Framing,
    end: End,
}

impl Reply {
    fn new(status: u16, content_type: &'static str, body: Arc<[u8]>) -> Self {
        Self {
            status,
            content_type,
            challenge: None,
            body,
            framing: Framing::Length,
            end: End::Close,
        }
    }

    fn framed(mut self, framing: Framing, end: End) -> Self {
        self.framing = framing;
        self.end = end;
        self
    }
}

struct RawServer {
    uri: Arc<str>,
    task: JoinHandle<()>,
}

impl Drop for RawServer {
    fn drop(&mut self) {
        // Dropping the JoinSet inside the task aborts every connection.
        self.task.abort();
    }
}

async fn serve(reply: Reply) -> RawServer {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let reply = Arc::new(reply);
    let task = tokio::spawn(async move {
        let mut connections = JoinSet::new();
        while let Ok((stream, _)) = listener.accept().await {
            connections.spawn(respond(stream, reply.clone()));
        }
    });
    RawServer {
        uri: Arc::from(format!("http://{address}/mcp")),
        task,
    }
}

async fn respond(mut stream: TcpStream, reply: Arc<Reply>) {
    if read_request(&mut stream).await.is_err() {
        return;
    }
    // A client that stops reading may reset the connection mid-write.
    let _ = write_reply(&mut stream, &reply).await;
    if matches!(reply.end, End::Hold) {
        std::future::pending::<()>().await;
    }
}

/// Reads one request head and its `Content-Length` body.
async fn read_request(stream: &mut TcpStream) -> io::Result<()> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(index) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let head = String::from_utf8_lossy(&buffer[..head_end]).to_ascii_lowercase();
    let body_len = head
        .lines()
        .find_map(|line| line.strip_prefix("content-length:"))
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    while buffer.len() < head_end + body_len {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    Ok(())
}

async fn write_reply(stream: &mut TcpStream, reply: &Reply) -> io::Result<()> {
    let mut head = format!(
        "HTTP/1.1 {} Test\r\ncontent-type: {}\r\nretry-after: 17\r\n",
        reply.status, reply.content_type
    );
    if let Some(challenge) = reply.challenge {
        head.push_str(&format!("www-authenticate: {challenge}\r\n"));
    }
    match reply.framing {
        Framing::Length => head.push_str(&format!("content-length: {}\r\n", reply.body.len())),
        Framing::Declared(len) => head.push_str(&format!("content-length: {len}\r\n")),
        Framing::Chunked => head.push_str("transfer-encoding: chunked\r\n"),
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).await?;
    let chunked = matches!(reply.framing, Framing::Chunked);
    for piece in reply.body.chunks(WRITE_CHUNK) {
        if chunked {
            stream
                .write_all(format!("{:x}\r\n", piece.len()).as_bytes())
                .await?;
            stream.write_all(piece).await?;
            stream.write_all(b"\r\n").await?;
        } else {
            stream.write_all(piece).await?;
        }
    }
    if chunked && matches!(reply.end, End::Close) {
        stream.write_all(b"0\r\n\r\n").await?;
    }
    stream.flush().await
}

async fn post(server: &RawServer, message: Message, session: Option<&str>) -> PostResult {
    tokio::time::timeout(
        DEADLINE,
        reqwest::Client::new().post_message(
            server.uri.clone(),
            message.build(),
            session.map(Arc::from),
            None,
            HashMap::new(),
        ),
    )
    .await
    .expect("the POST must finish without reading past the body limit")
}

/// Bounded `Debug` output, so a failure never dumps a large body.
fn summary(value: &impl Debug) -> String {
    const MAX: usize = 300;
    let rendered = format!("{value:?}");
    if rendered.len() <= MAX {
        return rendered;
    }
    let mut end = MAX;
    while !rendered.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}... ({} bytes)", &rendered[..end], rendered.len())
}

#[derive(Clone, Copy, Debug)]
enum Outcome {
    TooLarge(usize),
    Status(u16),
    JsonResult,
    JsonError(i32),
    Unexpected,
    Accepted,
    SessionExpired,
    AuthRequired,
}

fn assert_outcome(result: PostResult, expected: Outcome) {
    let rendered = summary(&result);
    match (expected, result) {
        (
            Outcome::TooLarge(limit),
            Err(error @ StreamableHttpError::ResponseBodyTooLarge { max_size, .. }),
        ) if max_size == limit => {
            assert_eq!(
                error.to_string(),
                format!("HTTP response body exceeded the maximum size of {limit} bytes")
            );
        }
        (Outcome::Status(status), Err(StreamableHttpError::HttpStatus(metadata)))
            if metadata.status == status =>
        {
            assert_eq!(metadata.retry_after.as_deref(), Some("17"));
        }
        (
            Outcome::JsonResult,
            Ok(StreamableHttpPostResponse::Json(JsonRpcMessage::Response(response), _)),
        ) => assert_eq!(response.id, RequestId::Number(1)),
        (
            Outcome::JsonError(code),
            Ok(StreamableHttpPostResponse::Json(JsonRpcMessage::Error(error), _)),
        ) if error.error.code.0 == code => assert_eq!(error.id, Some(RequestId::Number(1))),
        (Outcome::Unexpected, Err(StreamableHttpError::UnexpectedServerResponse(_)))
        | (Outcome::Accepted, Ok(StreamableHttpPostResponse::Accepted))
        | (Outcome::SessionExpired, Err(StreamableHttpError::SessionExpired))
        | (Outcome::AuthRequired, Err(StreamableHttpError::AuthRequired(_))) => {}
        (expected, _) => panic!("expected {expected:?}, got {rendered}"),
    }
}

/// Valid JSON followed by whitespace past the limit is rejected, so the
/// limit is enforced on bytes read rather than on what parses. Held-open
/// bodies prove the read stops at the limit.
#[rstest::rstest]
#[case::request_content_length(Message::Request, Payload::Result, Framing::Length, End::Close)]
#[case::request_chunked(Message::Request, Payload::Result, Framing::Chunked, End::Close)]
#[case::request_chunked_held_open(Message::Request, Payload::Result, Framing::Chunked, End::Hold)]
#[case::request_declared_held_open(
    Message::Request,
    Payload::Result,
    Framing::Declared(HUGE_DECLARED),
    End::Hold
)]
#[case::notification_content_length(
    Message::Notification,
    Payload::Result,
    Framing::Length,
    End::Close
)]
#[case::notification_chunked_held_open(
    Message::Notification,
    Payload::Result,
    Framing::Chunked,
    End::Hold
)]
#[case::notification_unparsable_not_accepted(
    Message::Notification,
    Payload::Text,
    Framing::Length,
    End::Close
)]
#[tokio::test]
async fn oversized_json_success_fails_closed(
    #[case] message: Message,
    #[case] payload: Payload,
    #[case] framing: Framing,
    #[case] end: End,
) {
    let body = payload.bytes(MAX_JSON_BODY + 1);
    let server = serve(Reply::new(200, "application/json", body).framed(framing, end)).await;
    assert_outcome(
        post(&server, message, None).await,
        Outcome::TooLarge(MAX_JSON_BODY),
    );
}

#[tokio::test]
async fn raising_the_sse_limit_does_not_disable_the_json_body_limit() {
    let server = serve(Reply::new(
        200,
        "application/json",
        Payload::Result.bytes(MAX_JSON_BODY + 1),
    ))
    .await;
    let result = tokio::time::timeout(
        DEADLINE,
        reqwest::Client::new().post_message_with_max_sse_event_size(
            server.uri.clone(),
            Message::Request.build(),
            None,
            None,
            HashMap::new(),
            usize::MAX,
        ),
    )
    .await
    .expect("JSON response size must not depend on the SSE limit");
    assert_outcome(result, Outcome::TooLarge(MAX_JSON_BODY));
}

#[rstest::rstest]
#[case::exact_content_length(Message::Request, MAX_JSON_BODY, Framing::Length)]
#[case::exact_chunked(Message::Request, MAX_JSON_BODY, Framing::Chunked)]
#[case::under_content_length(Message::Request, MAX_JSON_BODY - 1, Framing::Length)]
#[case::notification_exact(Message::Notification, MAX_JSON_BODY, Framing::Chunked)]
#[tokio::test]
async fn json_success_at_or_under_the_limit_is_parsed(
    #[case] message: Message,
    #[case] len: usize,
    #[case] framing: Framing,
) {
    let body = Payload::Result.bytes(len);
    let server = serve(Reply::new(200, "application/json", body).framed(framing, End::Close)).await;
    assert_outcome(post(&server, message, None).await, Outcome::JsonResult);
}

#[rstest::rstest]
#[case::json_rpc_error(
    Message::Request,
    "application/json",
    Payload::Error,
    Framing::Length,
    End::Close
)]
#[case::json_rpc_error_chunked_held_open(
    Message::Request,
    "application/json",
    Payload::Error,
    Framing::Chunked,
    End::Hold
)]
#[case::plain_text(
    Message::Request,
    "text/plain",
    Payload::Text,
    Framing::Length,
    End::Close
)]
#[case::plain_text_chunked_held_open(
    Message::Request,
    "text/plain",
    Payload::Text,
    Framing::Chunked,
    End::Hold
)]
#[case::discover_json_rpc_error(
    Message::Discover,
    "application/json",
    Payload::Error,
    Framing::Length,
    End::Close
)]
#[case::discover_plain_text(
    Message::Discover,
    "text/plain",
    Payload::Text,
    Framing::Chunked,
    End::Hold
)]
#[case::notification_json_rpc_error(
    Message::Notification,
    "application/json",
    Payload::Error,
    Framing::Length,
    End::Close
)]
#[tokio::test]
async fn oversized_client_error_bodies_fail_closed(
    #[case] message: Message,
    #[case] content_type: &'static str,
    #[case] payload: Payload,
    #[case] framing: Framing,
    #[case] end: End,
) {
    let body = payload.bytes(MAX_ERROR_BODY + 1);
    let server = serve(Reply::new(400, content_type, body).framed(framing, end)).await;
    assert_outcome(
        post(&server, message, None).await,
        Outcome::TooLarge(MAX_ERROR_BODY),
    );
}

/// An oversized JSON 5xx body drops the JSON-RPC error but keeps the
/// body-free status metadata.
#[rstest::rstest]
#[case::content_length(500, Framing::Length, End::Close)]
#[case::chunked_held_open(503, Framing::Chunked, End::Hold)]
#[tokio::test]
async fn oversized_json_server_error_keeps_status_metadata(
    #[case] status: u16,
    #[case] framing: Framing,
    #[case] end: End,
) {
    let body = Payload::Error.bytes(MAX_ERROR_BODY + 1);
    let server = serve(Reply::new(status, "application/json", body).framed(framing, end)).await;
    assert_outcome(
        post(&server, Message::Request, None).await,
        Outcome::Status(status),
    );
}

/// A declared `Content-Length` over the limit fails before any body byte
/// arrives.
#[rstest::rstest]
#[case::success(
    200,
    "application/json",
    Message::Request,
    JSON_OVER,
    Outcome::TooLarge(MAX_JSON_BODY)
)]
#[case::success_huge(
    200,
    "application/json",
    Message::Request,
    HUGE_DECLARED,
    Outcome::TooLarge(MAX_JSON_BODY)
)]
#[case::success_notification(
    200,
    "application/json",
    Message::Notification,
    JSON_OVER,
    Outcome::TooLarge(MAX_JSON_BODY)
)]
#[case::client_error_json(
    400,
    "application/json",
    Message::Request,
    ERROR_OVER,
    Outcome::TooLarge(MAX_ERROR_BODY)
)]
#[case::client_error_text(
    400,
    "text/plain",
    Message::Request,
    HUGE_DECLARED,
    Outcome::TooLarge(MAX_ERROR_BODY)
)]
#[case::server_error_json(
    500,
    "application/json",
    Message::Request,
    HUGE_DECLARED,
    Outcome::Status(500)
)]
#[tokio::test]
async fn declared_oversize_returns_before_any_body_bytes(
    #[case] status: u16,
    #[case] content_type: &'static str,
    #[case] message: Message,
    #[case] declared: u64,
    #[case] expected: Outcome,
) {
    let server = serve(
        Reply::new(status, content_type, Payload::Text.bytes(0))
            .framed(Framing::Declared(declared), End::Hold),
    )
    .await;
    assert_outcome(post(&server, message, None).await, expected);
}

/// Error bodies exactly at the limit keep their existing meaning, including
/// the sessionless `server/discover` legacy fallback.
#[rstest::rstest]
#[case::json_rpc_error(
    400,
    "application/json",
    Message::Request,
    Payload::Error,
    Framing::Length,
    Outcome::JsonError(SERVER_ERROR)
)]
#[case::json_rpc_error_chunked(
    400,
    "application/json",
    Message::Request,
    Payload::Error,
    Framing::Chunked,
    Outcome::JsonError(SERVER_ERROR)
)]
#[case::server_json_rpc_error(
    500,
    "application/json",
    Message::Request,
    Payload::Error,
    Framing::Length,
    Outcome::JsonError(SERVER_ERROR)
)]
#[case::plain_text(
    400,
    "text/plain",
    Message::Request,
    Payload::Text,
    Framing::Length,
    Outcome::Unexpected
)]
#[case::discover_json_rpc_error(
    400,
    "application/json",
    Message::Discover,
    Payload::Error,
    Framing::Length,
    Outcome::JsonError(SERVER_ERROR)
)]
#[case::discover_plain_text(
    400,
    "text/plain",
    Message::Discover,
    Payload::Text,
    Framing::Chunked,
    Outcome::JsonError(INVALID_REQUEST)
)]
#[tokio::test]
async fn error_bodies_at_the_limit_keep_their_meaning(
    #[case] status: u16,
    #[case] content_type: &'static str,
    #[case] message: Message,
    #[case] payload: Payload,
    #[case] framing: Framing,
    #[case] expected: Outcome,
) {
    let body = payload.bytes(MAX_ERROR_BODY);
    let server = serve(Reply::new(status, content_type, body).framed(framing, End::Close)).await;
    assert_outcome(post(&server, message, None).await, expected);
}

/// Responses classified from status and headers alone never read the body,
/// so a huge declared body that never arrives does not matter.
#[rstest::rstest]
#[case::accepted(202, "application/json", None, None, Outcome::Accepted)]
#[case::session_not_found(
    404,
    "application/json",
    None,
    Some("session-1"),
    Outcome::SessionExpired
)]
#[case::auth_challenge(401, "application/json", Some(CHALLENGE), None, Outcome::AuthRequired)]
#[case::rate_limited_json(429, "application/json", None, None, Outcome::Status(429))]
#[case::server_error_text(503, "text/plain", None, None, Outcome::Status(503))]
#[tokio::test]
async fn body_free_responses_ignore_the_declared_size(
    #[case] status: u16,
    #[case] content_type: &'static str,
    #[case] challenge: Option<&'static str>,
    #[case] session: Option<&'static str>,
    #[case] expected: Outcome,
) {
    let mut reply = Reply::new(status, content_type, Payload::Text.bytes(0))
        .framed(Framing::Declared(HUGE_DECLARED), End::Hold);
    reply.challenge = challenge;
    let server = serve(reply).await;
    assert_outcome(post(&server, Message::Request, session).await, expected);
}

// Run this suite with and without `reqwest/charset`: Cargo can enable the
// dependency feature through another consumer without an rmcp feature flag.
#[rstest::rstest]
#[case::windows_1252("text/plain; charset=windows-1252", b"\xe9t\xe9 \x80")]
#[case::quoted_charset("text/plain; charset=\"windows-1252\"", b"\xe9t\xe9")]
#[case::shift_jis(
    "text/plain; charset=shift_jis",
    b"\x82\xb1\x82\xf1\x82\xc9\x82\xbf\x82\xcd"
)]
#[case::utf16_bom("text/plain; charset=windows-1252", b"\xff\xfec\0a\0f\0\xe9\0")]
#[case::utf8_bom("text/plain", b"\xef\xbb\xbf\xc3\xa9t\xc3\xa9")]
#[case::unknown_charset("text/plain; charset=unknown", b"\xc3\xa9t\xc3\xa9")]
#[case::invalid_utf8("text/plain; charset=utf-8", b"bad \xff")]
#[case::malformed_content_type("text/plain; charset=", b"\xc3\xa9t\xc3\xa9")]
#[case::empty("text/plain; charset=windows-1252", b"")]
#[case::json_error(
    "application/json; charset=windows-1252",
    b"{\"jsonrpc\":\"2.0\",\"id\":1,\"error\":{\"code\":-32603,\"message\":\"\xe9t\xe9\"}}"
)]
#[case::json_error_utf16_bom("application/json; charset=utf-16le", b"\xff\xfe{\0}\0")]
#[tokio::test]
async fn error_body_decoding_matches_reqwest_text(
    #[case] content_type: &'static str,
    #[case] bytes: &'static [u8],
) {
    let server = serve(Reply::new(400, content_type, Arc::from(bytes))).await;
    let client = reqwest::Client::new();
    let expected = client
        .post(server.uri.as_ref())
        .json(&Message::Request.build())
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let result = post(&server, Message::Request, None).await;
    if content_type.starts_with("application/json")
        && let Ok(JsonRpcMessage::Error(error)) =
            serde_json::from_str::<rmcp::model::ServerJsonRpcMessage>(&expected)
    {
        match result {
            Ok(StreamableHttpPostResponse::Json(JsonRpcMessage::Error(actual), _)) => {
                assert_eq!(actual.error, error.error);
            }
            other => panic!("expected decoded JSON-RPC error, got {}", summary(&other)),
        }
    } else {
        match result {
            Err(StreamableHttpError::UnexpectedServerResponse(actual)) => {
                assert_eq!(actual, format!("HTTP 400 Bad Request: {expected}"));
            }
            other => panic!("expected decoded HTTP error, got {}", summary(&other)),
        }
    }
}

#[rstest::rstest]
#[case::length(Framing::Length, End::Close)]
#[case::chunked(Framing::Chunked, End::Hold)]
#[tokio::test]
async fn oversized_charset_body_is_rejected_before_decoding(
    #[case] framing: Framing,
    #[case] end: End,
) {
    let body: Arc<[u8]> = vec![0x80; MAX_ERROR_BODY + 1].into();
    let server =
        serve(Reply::new(400, "text/plain; charset=windows-1252", body).framed(framing, end)).await;
    assert_outcome(
        post(&server, Message::Request, None).await,
        Outcome::TooLarge(MAX_ERROR_BODY),
    );
}

#[tokio::test]
async fn charset_limit_counts_input_bytes_before_text_expansion() {
    let server = serve(Reply::new(
        400,
        "text/plain; charset=windows-1252",
        vec![0x80; MAX_ERROR_BODY].into(),
    ))
    .await;
    let expected = reqwest::Client::new()
        .post(server.uri.as_ref())
        .json(&Message::Request.build())
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(expected.len() > MAX_ERROR_BODY);
    match post(&server, Message::Request, None).await {
        Err(StreamableHttpError::UnexpectedServerResponse(actual)) => {
            assert_eq!(actual, format!("HTTP 400 Bad Request: {expected}"));
        }
        other => panic!("expected decoded HTTP error, got {}", summary(&other)),
    }
}
