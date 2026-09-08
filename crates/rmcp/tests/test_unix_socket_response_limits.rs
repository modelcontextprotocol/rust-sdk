#![cfg(all(
    unix,
    feature = "transport-streamable-http-client-unix-socket",
    not(feature = "local")
))]

use std::{
    collections::HashMap,
    future::Future,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

use futures::StreamExt;
use rmcp::{
    model::{
        ClientJsonRpcMessage, ClientRequest, DiscoverRequest, DiscoverRequestParams,
        JsonRpcMessage, RequestId,
    },
    transport::{
        UnixSocketHttpClient,
        streamable_http_client::{
            StreamableHttpClient, StreamableHttpError, StreamableHttpPostResponse,
            StreamableHttpResponseLimits,
        },
    },
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixListener,
};

const URI: &str = "http://localhost/mcp";
const JSON: &str = r#"{"jsonrpc":"2.0","id":1,"result":{}}"#;

struct TestServer {
    path: PathBuf,
    task: tokio::task::JoinHandle<()>,
}

impl TestServer {
    fn new(responses: Vec<Vec<u8>>) -> Self {
        Self::with_eof(responses, true)
    }

    fn with_eof(responses: Vec<Vec<u8>>, finish_body: bool) -> Self {
        static NEXT_SOCKET: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "rmcp-body-limits-{}-{}.sock",
            std::process::id(),
            NEXT_SOCKET.fetch_add(1, Ordering::Relaxed),
        ));
        let listener = UnixListener::bind(&path).unwrap();
        let task = tokio::spawn(async move {
            for response in responses {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let mut buffer = [0; 1024];
                loop {
                    let count = stream.read(&mut buffer).await.unwrap();
                    assert_ne!(count, 0, "client closed before sending its request");
                    request.extend_from_slice(&buffer[..count]);
                    if let Some(end) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&request[..end]);
                        let length = headers
                            .lines()
                            .filter_map(|line| line.split_once(':'))
                            .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                            .map(|(_, value)| value.trim().parse::<usize>().unwrap())
                            .unwrap_or(0);
                        if request.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                // An oversized response may be rejected before all bytes are sent.
                let _ = stream.write_all(&response).await;
                if !finish_body {
                    // Keep the body open until the client drops the response.
                    let _ = stream.read_u8().await;
                }
            }
        });
        Self { path, task }
    }

    fn client(&self) -> UnixSocketHttpClient {
        UnixSocketHttpClient::new(self.path.to_str().unwrap(), URI)
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.task.abort();
        let _ = std::fs::remove_file(&self.path);
    }
}

fn response(status: &str, content_type: &str, body: &[u8], chunked: bool) -> Vec<u8> {
    let mut wire =
        format!("HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nConnection: close\r\n")
            .into_bytes();
    if chunked {
        wire.extend_from_slice(b"Transfer-Encoding: chunked\r\n\r\n");
        for chunk in body.chunks(7) {
            wire.extend_from_slice(format!("{:x}\r\n", chunk.len()).as_bytes());
            wire.extend_from_slice(chunk);
            wire.extend_from_slice(b"\r\n");
        }
        wire.extend_from_slice(b"0\r\n\r\n");
    } else {
        wire.extend_from_slice(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes());
        wire.extend_from_slice(body);
    }
    wire
}

fn message(method: &str) -> ClientJsonRpcMessage {
    if method == "server/discover" {
        return ClientJsonRpcMessage::request(
            ClientRequest::DiscoverRequest(DiscoverRequest::new(DiscoverRequestParams {})),
            RequestId::Number(1),
        );
    }
    serde_json::from_value(serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": method
    }))
    .unwrap()
}

fn limits(json: usize, error: usize) -> StreamableHttpResponseLimits {
    let mut limits = StreamableHttpResponseLimits::default();
    limits.max_json_response_size = json;
    limits.max_error_response_size = error;
    limits
}

async fn within_deadline<F: Future>(future: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(2), future)
        .await
        .expect("Unix socket response operation exceeded its two-second deadline")
}

#[tokio::test]
async fn json_thresholds_cover_content_length_chunked_and_recovery() {
    let limit = JSON.len() + 8;
    for chunked in [false, true] {
        let lengths = [limit - 1, limit, limit + 1, JSON.len()];
        let server = TestServer::new(
            lengths
                .iter()
                .map(|&length| {
                    let body = format!("{JSON}{}", " ".repeat(length - JSON.len()));
                    response("200 OK", "application/json", body.as_bytes(), chunked)
                })
                .collect(),
        );
        let client = server.client();
        for length in lengths {
            let result = within_deadline(client.post_message_with_response_limits(
                URI.into(),
                message("ping"),
                None,
                None,
                HashMap::new(),
                limits(limit, 1),
            ))
            .await;
            if length > limit {
                assert!(
                    matches!(result, Err(StreamableHttpError::ResponseBodyTooLarge { limit: actual }) if actual == limit)
                );
            } else {
                assert!(
                    matches!(result, Ok(StreamableHttpPostResponse::Json(..))),
                    "{result:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn diagnostic_prefixes_preserve_http_errors_and_discovery_fallback() {
    let limit = 32;
    for chunked in [false, true] {
        let lengths = [limit - 1, limit, limit + 1, limit + 1, 1];
        let server = TestServer::new(
            lengths
                .iter()
                .map(|&length| {
                    response(
                        "400 Bad Request",
                        "text/plain",
                        &vec![b'x'; length],
                        chunked,
                    )
                })
                .collect(),
        );
        let client = server.client();
        for (index, length) in lengths.into_iter().enumerate() {
            let result = within_deadline(client.post_message_with_response_limits(
                URI.into(),
                message(if index == 3 {
                    "server/discover"
                } else {
                    "ping"
                }),
                None,
                None,
                HashMap::new(),
                limits(1, limit),
            ))
            .await;
            if index == 3 {
                let Ok(StreamableHttpPostResponse::Json(JsonRpcMessage::Error(error), _)) = result
                else {
                    panic!("expected legacy discovery fallback");
                };
                assert!(error.error.message.ends_with(&"x".repeat(limit)));
            } else {
                assert!(
                    matches!(
                        result,
                        Err(StreamableHttpError::UnexpectedServerResponse(ref body))
                            if body == &format!("HTTP 400 Bad Request: {}", "x".repeat(length.min(limit)))
                    ),
                    "{result:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn oversized_invalid_json_is_not_accepted() {
    let server = TestServer::new(vec![response(
        "200 OK",
        "application/json",
        &[b'x'; 65],
        true,
    )]);
    let result = within_deadline(server.client().post_message_with_response_limits(
        URI.into(),
        message("ping"),
        None,
        None,
        HashMap::new(),
        limits(64, 64),
    ))
    .await;
    assert!(matches!(
        result,
        Err(StreamableHttpError::ResponseBodyTooLarge { limit: 64 })
    ));
}

#[tokio::test]
async fn legacy_post_methods_apply_default_json_limit_on_every_status() {
    for (status, content_type, limit) in [
        ("200 OK", "application/json", 64 * 1024 * 1024),
        ("400 Bad Request", "application/json", 64 * 1024 * 1024),
    ] {
        let header = format!(
            "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\r\n",
            limit + 1,
        )
        .into_bytes();
        let server = TestServer::new(vec![header.clone(), header]);
        let client = server.client();
        let first = within_deadline(client.post_message(
            URI.into(),
            message("ping"),
            None,
            None,
            HashMap::new(),
        ))
        .await;
        assert!(
            matches!(first, Err(StreamableHttpError::ResponseBodyTooLarge { limit: actual }) if actual == limit)
        );
        let second = within_deadline(client.post_message_with_max_sse_event_size(
            URI.into(),
            message("ping"),
            None,
            None,
            HashMap::new(),
            128,
        ))
        .await;
        assert!(
            matches!(second, Err(StreamableHttpError::ResponseBodyTooLarge { limit: actual }) if actual == limit)
        );
    }
}

#[tokio::test]
async fn legacy_post_methods_truncate_diagnostics_and_keep_auth_session_precedence() {
    let limit = 64 * 1024;
    let body = vec![b'x'; limit + 1];
    let server = TestServer::new(vec![
        response("500 Internal Server Error", "text/plain", &body, false),
        response("500 Internal Server Error", "text/plain", &body, true),
    ]);
    let client = server.client();
    let results = [
        within_deadline(client.post_message(
            URI.into(),
            message("ping"),
            None,
            None,
            HashMap::new(),
        ))
        .await,
        within_deadline(client.post_message_with_max_sse_event_size(
            URI.into(),
            message("ping"),
            None,
            None,
            HashMap::new(),
            0,
        ))
        .await,
    ];
    for result in results {
        assert!(
            matches!(result, Err(StreamableHttpError::UnexpectedServerResponse(body))
            if body == format!("HTTP 500 Internal Server Error: {}", "x".repeat(limit)))
        );
    }

    for status in [
        "401 Unauthorized",
        "403 Forbidden",
        "404 Not Found",
        "500 Internal Server Error",
    ] {
        let server = TestServer::new(vec![response(status, "text/html", &body, true)]);
        let result = within_deadline(server.client().post_message_with_response_limits(
            URI.into(),
            message("server/discover"),
            if status.starts_with("404") {
                Some("expired".into())
            } else {
                None
            },
            None,
            HashMap::new(),
            limits(0, 0),
        ))
        .await;
        if status.starts_with("404") {
            assert!(matches!(result, Err(StreamableHttpError::SessionExpired)));
        } else {
            assert!(matches!(
                result,
                Err(StreamableHttpError::UnexpectedServerResponse(_))
            ));
        }
    }
    for status in ["401 Unauthorized", "403 Forbidden"] {
        let wire = format!("HTTP/1.1 {status}\r\nWWW-Authenticate: Bearer scope=\"read\"\r\nContent-Length: 999999\r\n\r\n").into_bytes();
        let server = TestServer::with_eof(vec![wire], false);
        let result = within_deadline(server.client().post_message_with_response_limits(
            URI.into(),
            message("server/discover"),
            None,
            None,
            HashMap::new(),
            limits(0, 0),
        ))
        .await;
        match status {
            "401 Unauthorized" => {
                assert!(matches!(result, Err(StreamableHttpError::AuthRequired(_))))
            }
            _ => assert!(matches!(
                result,
                Err(StreamableHttpError::InsufficientScope(_))
            )),
        }
    }
}

#[tokio::test]
async fn json_rpc_errors_use_json_limit_on_success_and_failure() {
    let body = br#"{"jsonrpc":"2.0","id":1,"error":{"code":-32600,"message":"real error"}}"#;
    for status in ["200 OK", "400 Bad Request"] {
        for chunked in [false, true] {
            for limit in [body.len() - 1, body.len(), body.len() + 1] {
                let server =
                    TestServer::new(vec![response(status, "application/json", body, chunked)]);
                let result = within_deadline(server.client().post_message_with_response_limits(
                    URI.into(),
                    message("server/discover"),
                    None,
                    None,
                    HashMap::new(),
                    limits(limit, 0),
                ))
                .await;
                if limit < body.len() {
                    assert!(matches!(
                        result,
                        Err(StreamableHttpError::ResponseBodyTooLarge { .. })
                    ));
                } else {
                    let Ok(StreamableHttpPostResponse::Json(JsonRpcMessage::Error(error), _)) =
                        result
                    else {
                        panic!(
                            "expected the complete JSON-RPC error, not a diagnostic or synthetic fallback"
                        );
                    };
                    assert_eq!(error.error.message, "real error");
                }
            }
        }
    }
}

#[tokio::test]
async fn diagnostic_prefix_does_not_wait_for_eof_or_parse_partial_json() {
    let prefix = br#"{"jsonrpc":"2.0","id":1,"error":{"code":-32600,"message":"prefix"}}"#;
    let mut body = prefix.to_vec();
    body.extend_from_slice(b"not-json");
    for content_type in ["text/plain", "application/json"] {
        let server = TestServer::new(vec![response("400 Bad Request", content_type, &body, true)]);
        let result = within_deadline(server.client().post_message_with_response_limits(
            URI.into(),
            message("ping"),
            None,
            None,
            HashMap::new(),
            limits(body.len(), prefix.len()),
        ))
        .await;
        assert!(matches!(
            result,
            Err(StreamableHttpError::UnexpectedServerResponse(_))
        ));
    }
    let limit = 64 * 1024;
    let mut wire = b"HTTP/1.1 422 Unprocessable Entity\r\nContent-Type: text/html\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
    wire.extend_from_slice(format!("{:x}\r\n{}\r\n", limit, "x".repeat(limit)).as_bytes());
    let server = TestServer::with_eof(vec![wire], false);
    let result = within_deadline(server.client().post_message(
        URI.into(),
        message("server/discover"),
        None,
        None,
        HashMap::new(),
    ))
    .await;
    let Ok(StreamableHttpPostResponse::Json(JsonRpcMessage::Error(error), _)) = result else {
        panic!("expected legacy fallback without waiting for another chunk or EOF");
    };
    assert!(error.error.message.ends_with(&"x".repeat(limit)));
}

#[tokio::test]
async fn response_limits_keep_sse_event_limit_independent() {
    let server = TestServer::new(vec![response(
        "200 OK",
        "text/event-stream",
        b"data: a response larger than the event limit\n\n",
        true,
    )]);
    let mut limits = limits(1, 1);
    limits.max_sse_event_size = 16;
    let result = within_deadline(server.client().post_message_with_response_limits(
        URI.into(),
        message("ping"),
        None,
        None,
        HashMap::new(),
        limits,
    ))
    .await
    .unwrap();
    let StreamableHttpPostResponse::Sse(mut stream, _) = result else {
        panic!("expected SSE response");
    };
    let error = within_deadline(stream.next())
        .await
        .expect("oversized SSE event must produce an error")
        .unwrap_err();
    let sse_stream::Error::Body(error) = error else {
        panic!("expected an SSE body-size error, got {error}");
    };
    assert_eq!(
        error.to_string(),
        "SSE event exceeded the maximum size of 16 bytes",
    );
}
