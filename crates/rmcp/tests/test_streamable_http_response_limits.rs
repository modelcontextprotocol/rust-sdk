#![cfg(all(
    feature = "client",
    feature = "transport-streamable-http-client-reqwest",
    not(feature = "local")
))]

use std::{collections::HashMap, convert::Infallible, sync::Arc, time::Duration};

use axum::{Router, body::Body, http::Response, routing::post as post_route};
use bytes::Bytes;
use futures::StreamExt;
use rmcp::{
    model::{
        ClientJsonRpcMessage, ClientRequest, DiscoverRequest, DiscoverRequestParams,
        JsonRpcMessage, PingRequest, RequestId,
    },
    transport::streamable_http_client::{
        StreamableHttpClient, StreamableHttpError, StreamableHttpPostResponse,
        StreamableHttpResponseLimits,
    },
};
use rstest::rstest;

const JSON_RESPONSE: &str = r#"{"jsonrpc":"2.0","id":1,"result":{}}"#;
const JSON_ERROR: &str =
    r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32600,"message":"Invalid Request"}}"#;

struct MockServer {
    uri: Arc<str>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl MockServer {
    async fn start(
        status: u16,
        content_type: &'static str,
        body: impl Into<Bytes>,
        chunked: bool,
    ) -> Self {
        Self::with_headers(status, content_type, body, chunked, Vec::new()).await
    }

    async fn with_headers(
        status: u16,
        content_type: &'static str,
        body: impl Into<Bytes>,
        chunked: bool,
        headers: Vec<(&'static str, &'static str)>,
    ) -> Self {
        let body = body.into();
        let router = Router::new().route(
            "/mcp",
            post_route(move || {
                let body = body.clone();
                let headers = headers.clone();
                async move {
                    let mut response = Response::builder()
                        .status(status)
                        .header("content-type", content_type)
                        .header("mcp-session-id", "test-session");
                    for (name, value) in headers {
                        response = response.header(name, value);
                    }
                    let body = if chunked {
                        // Unknown size forces chunked HTTP. Delay each chunk so the
                        // client must account for bytes across successive reads.
                        let chunks = body
                            .chunks((body.len() / 3).max(1))
                            .map(Bytes::copy_from_slice)
                            .collect::<Vec<_>>();
                        Body::from_stream(futures::stream::iter(chunks).then(|chunk| async move {
                            tokio::time::sleep(Duration::from_millis(1)).await;
                            Ok::<_, Infallible>(chunk)
                        }))
                    } else {
                        response = response.header("content-length", body.len());
                        Body::from(body)
                    };
                    response.body(body).unwrap()
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Self {
            uri: Arc::from(format!("http://{address}/mcp")),
            task,
        }
    }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap()
}

fn ping() -> ClientJsonRpcMessage {
    ClientJsonRpcMessage::request(
        ClientRequest::PingRequest(PingRequest::default()),
        RequestId::Number(1),
    )
}

fn discover() -> ClientJsonRpcMessage {
    ClientJsonRpcMessage::request(
        ClientRequest::DiscoverRequest(DiscoverRequest::new(DiscoverRequestParams {})),
        RequestId::Number(1),
    )
}

fn limits(json: usize, error: usize, sse: usize) -> StreamableHttpResponseLimits {
    let mut limits = StreamableHttpResponseLimits::default();
    limits.max_json_response_size = json;
    limits.max_error_response_size = error;
    limits.max_sse_event_size = sse;
    limits
}

async fn post(
    server: &MockServer,
    message: ClientJsonRpcMessage,
    limits: StreamableHttpResponseLimits,
) -> Result<StreamableHttpPostResponse, StreamableHttpError<reqwest::Error>> {
    client()
        .post_message_with_response_limits(
            server.uri.clone(),
            message,
            None,
            None,
            HashMap::new(),
            limits,
        )
        .await
}

#[rstest]
#[case::below_limit(1, false)]
#[case::exact_limit(0, false)]
#[case::over_limit(-1, false)]
#[case::chunked_below_limit(1, true)]
#[case::chunked_exact_limit(0, true)]
#[case::chunked_over_limit(-1, true)]
#[tokio::test]
async fn json_response_limit_is_inclusive(#[case] margin: isize, #[case] chunked: bool) {
    let server = MockServer::start(200, "application/json", JSON_RESPONSE, chunked).await;
    let limit = JSON_RESPONSE.len().checked_add_signed(margin).unwrap();
    let result = post(&server, ping(), limits(limit, 0, 0)).await;
    if margin < 0 {
        assert!(
            matches!(result, Err(StreamableHttpError::ResponseBodyTooLarge { limit: got }) if got == limit)
        );
    } else {
        let StreamableHttpPostResponse::Json(JsonRpcMessage::Response(_), session) =
            result.unwrap()
        else {
            panic!("expected JSON-RPC response");
        };
        assert_eq!(session.as_deref(), Some("test-session"));
    }
}

#[rstest]
#[case::fixed(false)]
#[case::chunked(true)]
#[tokio::test]
async fn oversized_malformed_json_is_not_accepted(#[case] chunked: bool) {
    let server = MockServer::start(200, "application/json", "not valid json", chunked).await;
    assert!(matches!(
        post(&server, ping(), limits(4, 100, 100)).await,
        Err(StreamableHttpError::ResponseBodyTooLarge { limit: 4 })
    ));
    // Preserve the existing malformed-response fallback within the bound.
    assert!(matches!(
        post(&server, ping(), limits(100, 100, 100)).await,
        Ok(StreamableHttpPostResponse::Accepted)
    ));
}

#[rstest]
#[case::below_limit(1, false)]
#[case::exact_limit(0, false)]
#[case::over_limit(-1, false)]
#[case::chunked_below_limit(1, true)]
#[case::chunked_exact_limit(0, true)]
#[case::chunked_over_limit(-1, true)]
#[tokio::test]
async fn json_rpc_error_uses_error_body_limit(#[case] margin: isize, #[case] chunked: bool) {
    let server = MockServer::start(400, "application/json", JSON_ERROR, chunked).await;
    let limit = JSON_ERROR.len().checked_add_signed(margin).unwrap();
    let result = post(&server, ping(), limits(0, limit, 0)).await;
    if margin < 0 {
        assert!(
            matches!(result, Err(StreamableHttpError::ResponseBodyTooLarge { limit: got }) if got == limit)
        );
    } else {
        let StreamableHttpPostResponse::Json(JsonRpcMessage::Error(error), session) =
            result.unwrap()
        else {
            panic!("expected JSON-RPC error");
        };
        assert_eq!(error.error.message, "Invalid Request");
        assert_eq!(session.as_deref(), Some("test-session"));
    }
}

#[rstest]
#[case::fixed(false)]
#[case::chunked(true)]
#[tokio::test]
async fn oversized_discovery_rejection_cannot_trigger_legacy_fallback(#[case] chunked: bool) {
    let body = "Unexpected message, expect initialize request";
    let server = MockServer::start(422, "text/plain", body, chunked).await;
    assert!(matches!(
        post(&server, discover(), limits(100, body.len() - 1, 100)).await,
        Err(StreamableHttpError::ResponseBodyTooLarge { .. })
    ));
    assert!(matches!(
        post(&server, discover(), limits(100, body.len(), 100)).await,
        Ok(StreamableHttpPostResponse::Json(
            JsonRpcMessage::Error(_),
            _
        ))
    ));
}

#[rstest]
#[case::fixed(false)]
#[case::chunked(true)]
#[tokio::test]
async fn non_json_error_bodies_are_bounded(#[case] chunked: bool) {
    let server = MockServer::start(500, "text/plain", "failure", chunked).await;
    assert!(matches!(
        post(&server, ping(), limits(100, 6, 100)).await,
        Err(StreamableHttpError::ResponseBodyTooLarge { limit: 6 })
    ));
    assert!(matches!(
        post(&server, ping(), limits(0, 7, 0)).await,
        Err(StreamableHttpError::UnexpectedServerResponse(message))
            if message.contains("failure")
    ));
}

#[rstest]
#[case::accepted(202)]
#[case::no_content(204)]
#[tokio::test]
async fn accepted_statuses_do_not_require_a_response_body(#[case] status: u16) {
    let server = MockServer::start(status, "application/json", "", false).await;
    assert!(matches!(
        post(&server, ping(), limits(0, 0, 0)).await,
        Ok(StreamableHttpPostResponse::Accepted)
    ));
}

#[tokio::test]
async fn sse_responses_keep_their_independent_event_limit() {
    let server = MockServer::start(200, "text/event-stream", "data: example\n\n", true).await;
    let StreamableHttpPostResponse::Sse(mut stream, _) =
        post(&server, ping(), limits(0, 0, 64)).await.unwrap()
    else {
        panic!("expected SSE stream");
    };
    assert!(stream.next().await.unwrap().is_ok());

    let StreamableHttpPostResponse::Sse(mut stream, _) =
        post(&server, ping(), limits(100, 100, 4)).await.unwrap()
    else {
        panic!("expected bounded SSE stream");
    };
    assert!(stream.next().await.unwrap().is_err());
}

#[rstest]
#[case::unauthorized(401)]
#[case::forbidden(403)]
#[tokio::test]
async fn authentication_challenges_keep_precedence(#[case] status: u16) {
    let server = MockServer::with_headers(
        status,
        "text/plain",
        "authentication required",
        false,
        vec![("www-authenticate", "Bearer scope=\"read\"")],
    )
    .await;
    let result = post(&server, ping(), limits(0, 0, 0)).await;
    match status {
        401 => assert!(matches!(result, Err(StreamableHttpError::AuthRequired(_)))),
        403 => assert!(matches!(
            result,
            Err(StreamableHttpError::InsufficientScope(_))
        )),
        _ => unreachable!(),
    }
}

#[tokio::test]
async fn expired_sessions_keep_precedence() {
    let server = MockServer::start(404, "text/plain", "session missing", false).await;
    let result = client()
        .post_message_with_response_limits(
            server.uri.clone(),
            ping(),
            Some(Arc::from("expired-session")),
            None,
            HashMap::new(),
            limits(0, 0, 0),
        )
        .await;
    assert!(matches!(result, Err(StreamableHttpError::SessionExpired)));
}

#[rstest]
#[case::post_message(false)]
#[case::post_message_with_max_sse_event_size(true)]
#[tokio::test]
async fn existing_entry_points_apply_default_body_limits(#[case] sse_limit_method: bool) {
    let limit = StreamableHttpResponseLimits::default().max_error_response_size;
    let server = MockServer::start(500, "text/plain", vec![b'x'; limit + 1], true).await;
    let client = client();
    let result = if sse_limit_method {
        client
            .post_message_with_max_sse_event_size(
                server.uri.clone(),
                ping(),
                None,
                None,
                HashMap::new(),
                usize::MAX,
            )
            .await
    } else {
        client
            .post_message(server.uri.clone(), ping(), None, None, HashMap::new())
            .await
    };
    assert!(
        matches!(result, Err(StreamableHttpError::ResponseBodyTooLarge { limit: got }) if got == limit)
    );
}
