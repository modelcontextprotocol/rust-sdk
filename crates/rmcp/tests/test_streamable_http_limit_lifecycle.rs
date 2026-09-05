#![cfg(all(
    feature = "client",
    feature = "transport-streamable-http-client-reqwest",
    not(feature = "local")
))]

use std::{
    collections::HashMap,
    convert::Infallible,
    io,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{Router, body::Body, http::Response, routing::post};
use bytes::Bytes;
use futures::{StreamExt, stream::BoxStream};
use http::{HeaderName, HeaderValue};
use rmcp::{
    ServiceExt,
    model::{
        CancelledNotificationParam, ClientInfo, ClientJsonRpcMessage, ClientRequest,
        DiscoverResult, PingRequest, ProtocolVersion, RequestId, ServerJsonRpcMessage,
    },
    service::{ClientInitializeError, ClientLifecycleMode, ClientServiceExt},
    transport::streamable_http_client::{
        StreamableHttpClient, StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
        StreamableHttpError, StreamableHttpPostResponse, StreamableHttpResponseLimits,
    },
};
use rstest::rstest;
use serde_json::{Value, json};

const TIMEOUT: Duration = Duration::from_secs(2);
const LIMITS: (usize, usize, usize) = (111, 222, 333);

#[derive(Debug)]
struct RecordedPost {
    method: String,
    id: Value,
    session: Option<Arc<str>>,
    limits: (usize, usize, usize),
}

#[derive(Default)]
struct RecordingState {
    posts: Vec<RecordedPost>,
    initializations: usize,
    pings: usize,
}

#[derive(Clone)]
struct RecordingClient {
    modern: bool,
    state: Arc<Mutex<RecordingState>>,
}

impl StreamableHttpClient for RecordingClient {
    type Error = io::Error;

    async fn post_message(
        &self,
        _uri: Arc<str>,
        _message: ClientJsonRpcMessage,
        _session_id: Option<Arc<str>>,
        _auth_header: Option<String>,
        _custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>> {
        panic!("transport bypassed the configured response limits")
    }

    async fn post_message_with_response_limits(
        &self,
        _uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        _auth_header: Option<String>,
        _custom_headers: HashMap<HeaderName, HeaderValue>,
        limits: StreamableHttpResponseLimits,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>> {
        let message = serde_json::to_value(message).unwrap();
        let method = message["method"].as_str().unwrap();
        let id = message["id"].clone();
        let mut state = self.state.lock().unwrap();
        state.posts.push(RecordedPost {
            method: method.to_owned(),
            id: id.clone(),
            session: session_id,
            limits: (
                limits.max_sse_event_size,
                limits.max_json_response_size,
                limits.max_error_response_size,
            ),
        });
        let (response, session) = match method {
            "server/discover" if self.modern => (
                json!({
                    "jsonrpc": "2.0", "id": id,
                    "result": DiscoverResult::new(
                        vec![ProtocolVersion::V_2026_07_28], Default::default()
                    ),
                }),
                None,
            ),
            "server/discover" => (
                json!({
                    "jsonrpc": "2.0", "id": id,
                    "error": { "code": -32601, "message": "legacy server" },
                }),
                None,
            ),
            "initialize" => {
                state.initializations += 1;
                (
                    json!({
                        "jsonrpc": "2.0", "id": id,
                        "result": {
                            "protocolVersion": "2025-11-25", "capabilities": {},
                            "serverInfo": { "name": "limit-recorder", "version": "1" },
                        },
                    }),
                    Some(format!("session-{}", state.initializations)),
                )
            }
            "notifications/initialized" | "notifications/cancelled" => {
                return Ok(StreamableHttpPostResponse::Accepted);
            }
            "ping" => {
                state.pings += 1;
                if !self.modern && state.pings == 1 {
                    return Err(StreamableHttpError::SessionExpired);
                }
                (json!({ "jsonrpc": "2.0", "id": id, "result": {} }), None)
            }
            other => panic!("unexpected POST method: {other}"),
        };
        Ok(StreamableHttpPostResponse::Json(
            serde_json::from_value::<ServerJsonRpcMessage>(response).unwrap(),
            session,
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
        _session_id: Option<Arc<str>>,
        _last_event_id: Option<String>,
        _auth_header: Option<String>,
        _custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<
        BoxStream<'static, Result<sse_stream::Sse, sse_stream::Error>>,
        StreamableHttpError<Self::Error>,
    > {
        Ok(Box::pin(futures::stream::pending()))
    }
}

fn auto_lifecycle() -> ClientLifecycleMode {
    ClientLifecycleMode::Auto {
        preferred_versions: vec![ProtocolVersion::V_2026_07_28],
        legacy_version: Some(ProtocolVersion::V_2025_11_25),
    }
}

/// An existing client implements only the pre-existing SSE-limited entry point.
#[derive(Clone, Default)]
struct LegacySseOnlyClient(Arc<Mutex<Vec<usize>>>);

impl StreamableHttpClient for LegacySseOnlyClient {
    type Error = io::Error;

    async fn post_message(
        &self,
        _uri: Arc<str>,
        _message: ClientJsonRpcMessage,
        _session_id: Option<Arc<str>>,
        _auth_header: Option<String>,
        _custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>> {
        panic!("new limits entry point bypassed the old SSE override")
    }

    async fn post_message_with_max_sse_event_size(
        &self,
        _uri: Arc<str>,
        _message: ClientJsonRpcMessage,
        _session_id: Option<Arc<str>>,
        _auth_header: Option<String>,
        _custom_headers: HashMap<HeaderName, HeaderValue>,
        max_sse_event_size: usize,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>> {
        self.0.lock().unwrap().push(max_sse_event_size);
        Ok(StreamableHttpPostResponse::Accepted)
    }

    async fn delete_session(
        &self,
        _uri: Arc<str>,
        _session_id: Arc<str>,
        _auth_header: Option<String>,
        _custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<(), StreamableHttpError<Self::Error>> {
        unreachable!("compatibility test does not create a session")
    }

    async fn get_stream(
        &self,
        _uri: Arc<str>,
        _session_id: Option<Arc<str>>,
        _last_event_id: Option<String>,
        _auth_header: Option<String>,
        _custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<
        BoxStream<'static, Result<sse_stream::Sse, sse_stream::Error>>,
        StreamableHttpError<Self::Error>,
    > {
        unreachable!("compatibility test does not open a stream")
    }
}

#[tokio::test]
async fn new_limits_entry_point_preserves_legacy_sse_override() {
    let client = LegacySseOnlyClient::default();
    let mut limits = StreamableHttpResponseLimits::default();
    limits.max_sse_event_size = LIMITS.0;
    limits.max_json_response_size = LIMITS.1;
    limits.max_error_response_size = LIMITS.2;
    let response = client
        .post_message_with_response_limits(
            Arc::from("http://127.0.0.1/record-only"),
            ClientJsonRpcMessage::request(
                ClientRequest::PingRequest(PingRequest::default()),
                RequestId::Number(1),
            ),
            None,
            None,
            HashMap::new(),
            limits,
        )
        .await
        .unwrap();
    assert!(matches!(response, StreamableHttpPostResponse::Accepted));
    assert_eq!(client.0.lock().unwrap().as_slice(), [LIMITS.0]);
}

#[rstest]
#[case::legacy(false, false)]
#[case::discover_fallback(true, false)]
#[case::modern_discover(true, true)]
#[tokio::test]
async fn response_limits_reach_every_lifecycle_post(
    #[case] auto: bool,
    #[case] modern: bool,
) -> anyhow::Result<()> {
    let recorder = RecordingClient {
        modern,
        state: Arc::default(),
    };
    let transport = StreamableHttpClientTransport::with_client(
        recorder.clone(),
        StreamableHttpClientTransportConfig::with_uri("http://127.0.0.1/record-only")
            .max_sse_event_size(LIMITS.0)
            .max_json_response_size(LIMITS.1)
            .max_error_response_size(LIMITS.2)
            .reinit_on_expired_session(true),
    );
    let client = if auto {
        tokio::time::timeout(
            TIMEOUT,
            ClientInfo::default().serve_with_lifecycle(transport, auto_lifecycle()),
        )
        .await??
    } else {
        tokio::time::timeout(TIMEOUT, ClientInfo::default().serve(transport)).await??
    };

    tokio::time::timeout(
        TIMEOUT,
        client.send_request(ClientRequest::PingRequest(PingRequest::default())),
    )
    .await??;
    if !modern {
        tokio::time::timeout(
            TIMEOUT,
            client.notify_cancelled(CancelledNotificationParam::new(
                Some(RequestId::Number(999)),
                None,
            )),
        )
        .await??;
    }
    tokio::time::timeout(TIMEOUT, client.cancel()).await??;

    let state = recorder.state.lock().unwrap();
    let expected = if modern {
        vec!["server/discover", "ping"]
    } else {
        let mut expected = vec![
            "initialize",
            "notifications/initialized",
            "ping",
            "initialize",
            "notifications/initialized",
            "ping",
            "notifications/cancelled",
        ];
        if auto {
            expected.insert(0, "server/discover");
        }
        expected
    };
    assert_eq!(
        state
            .posts
            .iter()
            .map(|post| post.method.as_str())
            .collect::<Vec<_>>(),
        expected,
    );
    for post in &state.posts {
        assert_eq!(post.limits, LIMITS, "limits changed for {post:?}");
    }
    if !modern {
        let pings = state
            .posts
            .iter()
            .filter(|post| post.method == "ping")
            .collect::<Vec<_>>();
        assert_eq!(pings[0].session.as_deref(), Some("session-1"));
        assert_eq!(pings[1].session.as_deref(), Some("session-2"));
        assert_eq!(
            pings[0].id, pings[1].id,
            "recovery must retry the original request"
        );
        for initialize in state
            .posts
            .iter()
            .filter(|post| post.method == "initialize")
        {
            assert!(
                initialize.session.is_none(),
                "initialize must start a new session"
            );
        }
    }
    Ok(())
}

struct LoopbackServer {
    uri: String,
    posts: Arc<Mutex<Vec<String>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for LoopbackServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl LoopbackServer {
    async fn oversized_discover(error_response: bool, never_finishes: bool) -> Self {
        let posts = Arc::new(Mutex::new(Vec::new()));
        let recorded_posts = posts.clone();
        let router = Router::new().route(
            "/mcp",
            post(move |request: Bytes| {
                let posts = recorded_posts.clone();
                async move {
                    let request: Value = serde_json::from_slice(&request).unwrap();
                    posts
                        .lock()
                        .unwrap()
                        .push(request["method"].as_str().unwrap().to_owned());
                    let (status, content_type, payload) = if error_response {
                        (
                            422,
                            "text/plain",
                            "Unexpected message, expect initialize request".repeat(8),
                        )
                    } else {
                        (
                            200,
                            "application/json",
                            json!({
                                "jsonrpc": "2.0", "id": request["id"],
                                "error": { "code": -32601, "message": "legacy".repeat(80) },
                            })
                            .to_string(),
                        )
                    };
                    let body = if never_finishes {
                        // The limit must abort reading without waiting for EOF.
                        Body::from_stream(
                            futures::stream::once(async move {
                                Ok::<_, Infallible>(Bytes::from(payload))
                            })
                            .chain(futures::stream::pending()),
                        )
                    } else {
                        Body::from(payload)
                    };
                    Response::builder()
                        .status(status)
                        .header("content-type", content_type)
                        .body(body)
                        .unwrap()
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        Self {
            uri: format!("http://{address}/mcp"),
            posts,
            task,
        }
    }
}

#[rstest]
#[case::json(false, false)]
#[case::error(true, false)]
#[case::unfinished_json(false, true)]
#[case::unfinished_error(true, true)]
#[tokio::test]
async fn oversized_discover_fails_without_fallback_or_retry(
    #[case] error_response: bool,
    #[case] never_finishes: bool,
) {
    let server = LoopbackServer::oversized_discover(error_response, never_finishes).await;
    let transport = StreamableHttpClientTransport::with_client(
        reqwest::Client::builder().no_proxy().build().unwrap(),
        StreamableHttpClientTransportConfig::with_uri(server.uri.clone())
            .max_json_response_size(64)
            .max_error_response_size(64)
            .reinit_on_expired_session(true),
    );
    let result = tokio::time::timeout(
        TIMEOUT,
        ClientInfo::default().serve_with_lifecycle(transport, auto_lifecycle()),
    )
    .await
    .expect("oversized discovery must fail before EOF or the auto fallback timeout");
    let error = match result {
        Ok(client) => {
            client.cancel().await.unwrap();
            panic!("oversized discovery was accepted");
        }
        Err(error) => error,
    };
    let ClientInitializeError::TransportError { error, .. } = error else {
        panic!("expected the transport size error without lifecycle fallback: {error:?}");
    };
    assert!(
        matches!(
            error
                .error
                .downcast_ref::<StreamableHttpError<reqwest::Error>>(),
            Some(StreamableHttpError::ResponseBodyTooLarge { limit: 64 })
        ),
        "unexpected transport error: {error:?}"
    );
    assert_eq!(server.posts.lock().unwrap().as_slice(), ["server/discover"]);
}
