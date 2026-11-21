#![cfg(all(
    feature = "client",
    feature = "transport-streamable-http-client",
    not(feature = "local")
))]

use std::{
    collections::{HashMap, VecDeque},
    io,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use futures::{StreamExt, stream::BoxStream};
use http::{HeaderName, HeaderValue};
use rmcp::{
    ClientHandler, RoleClient, ServiceError,
    model::{
        ClientConfig, ClientJsonRpcMessage, ClientRequest, EmptyResult, PingRequest, RequestId,
        ServerJsonRpcMessage, ServerResult,
    },
    service::{
        ClientLifecycleMode, NotificationContext, RunningService, serve_client_with_lifecycle,
    },
    transport::{
        StreamableHttpClientTransport,
        common::client_side_sse::FixedInterval,
        streamable_http_client::{
            AuthRequiredError, InsufficientScopeError, StreamableHttpClient,
            StreamableHttpClientTransportConfig, StreamableHttpError, StreamableHttpPostResponse,
        },
    },
};
use serde_json::{Value, json};
use sse_stream::{Error as SseError, Sse};
use tokio::time::timeout;

const TEST_TIMEOUT: Duration = Duration::from_secs(1);
const RETRY_INTERVAL: Duration = Duration::from_millis(1);

#[derive(Clone, Copy)]
enum PingResponse {
    Json,
    DelayedJson(Duration),
    InterruptedSse { event_id: Option<&'static str> },
}

enum GetResponse {
    EmptySse,
    Unsupported,
    AuthRequired,
    InsufficientScope,
    SessionExpired,
    Transient,
    CompletePending,
    ToolsChanged,
}

struct ScriptState {
    get_responses: Mutex<VecDeque<GetResponse>>,
    ping_responses: Mutex<VecDeque<PingResponse>>,
    pending_request: Mutex<Option<RequestId>>,
    get_calls: AtomicUsize,
    ping_calls: AtomicUsize,
    initialize_calls: AtomicUsize,
    last_event_ids: Mutex<Vec<Option<String>>>,
    get_sessions: Mutex<Vec<Option<String>>>,
    ping_sessions: Mutex<Vec<Option<String>>>,
}

#[derive(Clone)]
struct ScriptedHttpClient {
    session_id: Option<&'static str>,
    replacement_session_id: Option<&'static str>,
    state: Arc<ScriptState>,
}

fn response(id: Value, result: Value) -> ServerJsonRpcMessage {
    serde_json::from_value(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
        .expect("scripted response is valid")
}

fn ping_response(id: RequestId) -> ServerJsonRpcMessage {
    ServerJsonRpcMessage::response(ServerResult::EmptyResult(EmptyResult {}), id)
}

impl StreamableHttpClient for ScriptedHttpClient {
    type Error = io::Error;

    async fn post_message(
        &self,
        _uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session: Option<Arc<str>>,
        _auth_header: Option<String>,
        _custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>> {
        let value = serde_json::to_value(message).expect("client message serializes");
        if value["method"].as_str() == Some("ping") {
            self.state
                .ping_sessions
                .lock()
                .expect("ping session IDs lock")
                .push(session.as_ref().map(|id| id.to_string()));
        }
        match value["method"].as_str() {
            Some("initialize") => {
                let call = self.state.initialize_calls.fetch_add(1, Ordering::SeqCst);
                let session_id = if call == 0 {
                    self.session_id
                } else {
                    self.replacement_session_id
                };
                Ok(StreamableHttpPostResponse::Json(
                    response(
                        value["id"].clone(),
                        json!({
                            "protocolVersion": "2025-11-25",
                            "capabilities": {},
                            "serverInfo": { "name": "scripted", "version": "1" },
                        }),
                    ),
                    session_id.map(str::to_owned),
                ))
            }
            Some("notifications/initialized") => Ok(StreamableHttpPostResponse::Accepted),
            Some("ping") => {
                self.state.ping_calls.fetch_add(1, Ordering::SeqCst);
                let id =
                    serde_json::from_value(value["id"].clone()).expect("ping request has an ID");
                let scripted = self
                    .state
                    .ping_responses
                    .lock()
                    .expect("ping response queue lock")
                    .pop_front()
                    .expect("ping response is scripted");
                match scripted {
                    PingResponse::Json => {
                        Ok(StreamableHttpPostResponse::Json(ping_response(id), None))
                    }
                    PingResponse::DelayedJson(delay) => {
                        tokio::time::sleep(delay).await;
                        Ok(StreamableHttpPostResponse::Json(ping_response(id), None))
                    }
                    PingResponse::InterruptedSse { event_id } => {
                        *self
                            .state
                            .pending_request
                            .lock()
                            .expect("pending request lock") = Some(id);
                        let stream = futures::stream::iter([Ok(Sse {
                            event: Some("checkpoint".to_owned()),
                            data: None,
                            id: event_id.map(str::to_owned),
                            retry: None,
                        })])
                        .boxed();
                        Ok(StreamableHttpPostResponse::Sse(stream, None))
                    }
                }
            }
            method => panic!("unexpected scripted method: {method:?}"),
        }
    }

    async fn delete_session(
        &self,
        _uri: Arc<str>,
        _session: Arc<str>,
        _auth_header: Option<String>,
        _custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<(), StreamableHttpError<Self::Error>> {
        Ok(())
    }

    async fn get_stream(
        &self,
        _uri: Arc<str>,
        session: Option<Arc<str>>,
        last_event_id: Option<String>,
        _auth_header: Option<String>,
        _custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<BoxStream<'static, Result<Sse, SseError>>, StreamableHttpError<Self::Error>> {
        self.state.get_calls.fetch_add(1, Ordering::SeqCst);
        self.state
            .last_event_ids
            .lock()
            .expect("last event IDs lock")
            .push(last_event_id);
        self.state
            .get_sessions
            .lock()
            .expect("GET session IDs lock")
            .push(session.map(|id| id.to_string()));
        let scripted = self
            .state
            .get_responses
            .lock()
            .expect("GET response queue lock")
            .pop_front()
            .unwrap_or(GetResponse::Transient);
        match scripted {
            GetResponse::EmptySse => Ok(futures::stream::empty().boxed()),
            GetResponse::Unsupported => Err(StreamableHttpError::ServerDoesNotSupportSse),
            GetResponse::AuthRequired => Err(StreamableHttpError::AuthRequired(
                AuthRequiredError::new("Bearer error=\"invalid_token\"".to_owned()),
            )),
            GetResponse::InsufficientScope => Err(StreamableHttpError::InsufficientScope(
                InsufficientScopeError::new(
                    "Bearer error=\"insufficient_scope\", scope=\"tools:read\"".to_owned(),
                    Some("tools:read".to_owned()),
                ),
            )),
            GetResponse::SessionExpired => Err(StreamableHttpError::SessionExpired),
            GetResponse::Transient => Err(StreamableHttpError::Client(io::Error::other(
                "transient reconnect failure",
            ))),
            GetResponse::CompletePending => {
                let id = self
                    .state
                    .pending_request
                    .lock()
                    .expect("pending request lock")
                    .take()
                    .expect("a request is waiting for its resumed response");
                let message = ping_response(id);
                Ok(futures::stream::iter([Ok(Sse {
                    event: Some("message".to_owned()),
                    data: Some(
                        serde_json::to_string(&message).expect("response message serializes"),
                    ),
                    id: Some("event-2".to_owned()),
                    retry: None,
                })])
                .boxed())
            }
            GetResponse::ToolsChanged => Ok(futures::stream::iter([Ok(Sse {
                event: Some("message".to_owned()),
                data: Some(
                    json!({ "jsonrpc": "2.0", "method": "notifications/tools/list_changed" })
                        .to_string(),
                ),
                id: Some("listener-event-1".to_owned()),
                retry: None,
            })])
            .chain(futures::stream::pending())
            .boxed()),
        }
    }
}

struct TestClient {
    tools_changed: Arc<tokio::sync::Notify>,
}

impl ClientHandler for TestClient {
    fn get_info(&self) -> ClientConfig {
        ClientConfig::default()
    }

    async fn on_tool_list_changed(&self, _context: NotificationContext<RoleClient>) {
        self.tools_changed.notify_one();
    }
}

struct Harness {
    client: RunningService<RoleClient, TestClient>,
    state: Arc<ScriptState>,
    tools_changed: Arc<tokio::sync::Notify>,
}

impl Harness {
    async fn start(
        session_id: Option<&'static str>,
        ping_responses: impl IntoIterator<Item = PingResponse>,
        get_responses: impl IntoIterator<Item = GetResponse>,
    ) -> anyhow::Result<Self> {
        Self::start_with(
            session_id,
            Some("session-2"),
            true,
            ping_responses,
            get_responses,
        )
        .await
    }

    async fn start_with(
        session_id: Option<&'static str>,
        replacement_session_id: Option<&'static str>,
        reinit_on_expired_session: bool,
        ping_responses: impl IntoIterator<Item = PingResponse>,
        get_responses: impl IntoIterator<Item = GetResponse>,
    ) -> anyhow::Result<Self> {
        let state = Arc::new(ScriptState {
            get_responses: Mutex::new(get_responses.into_iter().collect()),
            ping_responses: Mutex::new(ping_responses.into_iter().collect()),
            pending_request: Mutex::new(None),
            get_calls: AtomicUsize::new(0),
            ping_calls: AtomicUsize::new(0),
            initialize_calls: AtomicUsize::new(0),
            last_event_ids: Mutex::new(Vec::new()),
            get_sessions: Mutex::new(Vec::new()),
            ping_sessions: Mutex::new(Vec::new()),
        });
        let mut config = StreamableHttpClientTransportConfig::with_uri("http://scripted/mcp");
        let mut retry_config = FixedInterval::default();
        retry_config.max_times = None;
        retry_config.duration = RETRY_INTERVAL;
        config.retry_config = Arc::new(retry_config);
        config.auth_header = Some("Bearer test-token".to_owned());
        config.reinit_on_expired_session = reinit_on_expired_session;
        let transport = StreamableHttpClientTransport::with_client(
            ScriptedHttpClient {
                session_id,
                replacement_session_id,
                state: state.clone(),
            },
            config,
        );
        let tools_changed = Arc::new(tokio::sync::Notify::new());
        let client = serve_client_with_lifecycle(
            TestClient {
                tools_changed: tools_changed.clone(),
            },
            transport,
            ClientLifecycleMode::Initialize,
        )
        .await?;
        Ok(Self {
            client,
            state,
            tools_changed,
        })
    }

    async fn ping(&self) -> Result<ServerResult, rmcp::ServiceError> {
        self.client
            .send_request(ClientRequest::PingRequest(PingRequest::default()))
            .await
    }

    async fn wait_for_gets(&self, expected: usize) {
        timeout(TEST_TIMEOUT, async {
            while self.state.get_calls.load(Ordering::SeqCst) < expected {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("expected reconnect attempts");
    }

    async fn assert_get_count_stays(&self, expected: usize) {
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(self.state.get_calls.load(Ordering::SeqCst), expected);
    }

    fn last_event_ids(&self) -> Vec<Option<String>> {
        self.state
            .last_event_ids
            .lock()
            .expect("last event IDs lock")
            .clone()
    }

    fn get_sessions(&self) -> Vec<Option<String>> {
        self.state
            .get_sessions
            .lock()
            .expect("GET session IDs lock")
            .clone()
    }

    fn ping_sessions(&self) -> Vec<Option<String>> {
        self.state
            .ping_sessions
            .lock()
            .expect("ping session IDs lock")
            .clone()
    }

    async fn finish(self) -> anyhow::Result<()> {
        self.client.cancel().await?;
        Ok(())
    }
}

#[tokio::test]
async fn initial_listener_405_stays_optional_and_does_not_retry() -> anyhow::Result<()> {
    let harness = Harness::start(
        Some("session-1"),
        [PingResponse::Json],
        [GetResponse::Unsupported],
    )
    .await?;

    harness.wait_for_gets(1).await;
    harness.ping().await?;
    harness.assert_get_count_stays(1).await;
    harness.finish().await
}

#[tokio::test]
async fn established_listener_stops_after_reconnect_405_and_post_still_works() -> anyhow::Result<()>
{
    let harness = Harness::start(
        Some("session-1"),
        [PingResponse::Json],
        [GetResponse::EmptySse, GetResponse::Unsupported],
    )
    .await?;

    harness.wait_for_gets(2).await;
    harness.assert_get_count_stays(2).await;
    harness.ping().await?;
    assert_eq!(harness.last_event_ids(), vec![None, None]);
    harness.finish().await
}

#[tokio::test]
async fn interrupted_post_with_event_id_stops_after_reconnect_405() -> anyhow::Result<()> {
    let harness = Harness::start(
        None,
        [
            PingResponse::InterruptedSse {
                event_id: Some("event-1"),
            },
            PingResponse::Json,
        ],
        [GetResponse::Unsupported],
    )
    .await?;

    let first = timeout(TEST_TIMEOUT, harness.ping())
        .await
        .expect("interrupted request should fail promptly");
    assert!(first.is_err(), "interrupted request should fail after 405");
    harness.wait_for_gets(1).await;
    harness.assert_get_count_stays(1).await;
    assert_eq!(harness.last_event_ids(), vec![Some("event-1".to_owned())]);
    harness.ping().await?;
    harness.finish().await
}

#[tokio::test]
async fn established_listener_stops_after_reconnect_401() -> anyhow::Result<()> {
    assert_listener_auth_failure_stops(GetResponse::AuthRequired).await
}

#[tokio::test]
async fn established_listener_stops_after_reconnect_403() -> anyhow::Result<()> {
    assert_listener_auth_failure_stops(GetResponse::InsufficientScope).await
}

async fn assert_listener_auth_failure_stops(error: GetResponse) -> anyhow::Result<()> {
    let harness = Harness::start(
        Some("session-1"),
        [PingResponse::Json],
        [GetResponse::EmptySse, error],
    )
    .await?;
    harness.wait_for_gets(2).await;
    harness.assert_get_count_stays(2).await;
    harness.ping().await?;
    harness.assert_get_count_stays(2).await;
    harness.finish().await
}

#[tokio::test]
async fn interrupted_post_stops_after_reconnect_401_without_replaying_post() -> anyhow::Result<()> {
    assert_post_auth_failure_stops(GetResponse::AuthRequired).await
}

#[tokio::test]
async fn interrupted_post_stops_after_reconnect_403_without_replaying_post() -> anyhow::Result<()> {
    assert_post_auth_failure_stops(GetResponse::InsufficientScope).await
}

async fn assert_post_auth_failure_stops(error: GetResponse) -> anyhow::Result<()> {
    let harness = Harness::start(
        None,
        [
            PingResponse::InterruptedSse {
                event_id: Some("event-1"),
            },
            PingResponse::Json,
        ],
        [error],
    )
    .await?;
    let first = timeout(TEST_TIMEOUT, harness.ping())
        .await
        .expect("authorization failure should stop resuming promptly");
    assert!(
        first.is_err(),
        "request must fail instead of silently succeeding"
    );
    harness.assert_get_count_stays(1).await;
    assert_eq!(harness.state.ping_calls.load(Ordering::SeqCst), 1);
    assert_eq!(harness.last_event_ids(), vec![Some("event-1".to_owned())]);
    harness.ping().await?;
    assert_eq!(harness.state.ping_calls.load(Ordering::SeqCst), 2);
    harness.finish().await
}

#[tokio::test]
async fn interrupted_post_without_event_id_does_not_attempt_resume() -> anyhow::Result<()> {
    let harness = Harness::start(
        None,
        [
            PingResponse::InterruptedSse { event_id: None },
            PingResponse::Json,
        ],
        [],
    )
    .await?;

    let first = timeout(TEST_TIMEOUT, harness.ping())
        .await
        .expect("non-resumable request should fail promptly");
    assert!(
        first.is_err(),
        "request without a final response should fail"
    );
    harness.assert_get_count_stays(0).await;
    harness.ping().await?;
    harness.finish().await
}

#[tokio::test]
async fn interrupted_post_recovers_after_three_transient_failures_without_replaying_post()
-> anyhow::Result<()> {
    let harness = Harness::start(
        None,
        [PingResponse::InterruptedSse {
            event_id: Some("event-1"),
        }],
        [
            GetResponse::Transient,
            GetResponse::Transient,
            GetResponse::Transient,
            GetResponse::CompletePending,
        ],
    )
    .await?;

    timeout(TEST_TIMEOUT, harness.ping())
        .await
        .expect("transient reconnect should recover")?;
    harness.wait_for_gets(4).await;
    assert_eq!(harness.state.ping_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        harness.last_event_ids(),
        vec![Some("event-1".to_owned()); 4]
    );
    harness.finish().await
}

#[tokio::test]
async fn established_listener_delivers_notification_after_three_transient_failures()
-> anyhow::Result<()> {
    let harness = Harness::start(
        Some("session-1"),
        [PingResponse::Json],
        [
            GetResponse::EmptySse,
            GetResponse::Transient,
            GetResponse::Transient,
            GetResponse::Transient,
            GetResponse::ToolsChanged,
        ],
    )
    .await?;

    timeout(TEST_TIMEOUT, harness.tools_changed.notified())
        .await
        .expect("listener should recover and deliver a notification");
    assert_eq!(harness.state.get_calls.load(Ordering::SeqCst), 5);
    harness.ping().await?;
    harness.finish().await
}

#[tokio::test]
async fn interrupted_post_session_expired_fails_promptly_without_retrying() -> anyhow::Result<()> {
    // Negative control: without `SessionExpired` in the terminal set, the
    // unbounded retry policy keeps issuing resume GETs, so the ping below
    // times out and the GET count assertion fails.
    let harness = Harness::start_with(
        None,
        Some("session-2"),
        false,
        [PingResponse::InterruptedSse {
            event_id: Some("event-1"),
        }],
        [GetResponse::SessionExpired],
    )
    .await?;

    let error = timeout(TEST_TIMEOUT, harness.ping())
        .await
        .expect("session-expired resume should fail promptly")
        .expect_err("interrupted request cannot be replayed");
    assert!(matches!(&error, ServiceError::McpError(_)), "{error:?}");
    assert!(error.to_string().contains("Session expired"), "{error}");
    harness.assert_get_count_stays(1).await;
    assert_eq!(harness.state.initialize_calls.load(Ordering::SeqCst), 1);
    assert_eq!(harness.last_event_ids(), vec![Some("event-1".to_owned())]);
    harness.finish().await
}

#[tokio::test]
async fn interrupted_post_session_expired_reinitializes_and_restarts_listener() -> anyhow::Result<()>
{
    let harness = Harness::start_with(
        Some("session-1"),
        Some("session-2"),
        true,
        [PingResponse::InterruptedSse {
            event_id: Some("event-1"),
        }],
        [
            GetResponse::ToolsChanged,
            GetResponse::SessionExpired,
            GetResponse::ToolsChanged,
        ],
    )
    .await?;
    harness.wait_for_gets(1).await;

    let error = timeout(TEST_TIMEOUT, harness.ping())
        .await
        .expect("interrupted request should fail promptly")
        .expect_err("interrupted request cannot be replayed");
    assert!(error.to_string().contains("Session expired"), "{error}");
    harness.wait_for_gets(3).await;
    harness.assert_get_count_stays(3).await;
    assert_eq!(harness.state.initialize_calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        harness.get_sessions(),
        vec![
            Some("session-1".to_owned()),
            Some("session-1".to_owned()),
            Some("session-2".to_owned())
        ]
    );
    assert_eq!(
        harness.last_event_ids(),
        vec![None, Some("event-1".to_owned()), None]
    );
    harness.finish().await
}

#[tokio::test]
async fn stream_recovery_pauses_new_posts_until_the_replacement_session_is_ready()
-> anyhow::Result<()> {
    let harness = Harness::start_with(
        Some("session-1"),
        Some("session-2"),
        true,
        [
            PingResponse::DelayedJson(Duration::from_millis(80)),
            PingResponse::InterruptedSse {
                event_id: Some("event-1"),
            },
            PingResponse::Json,
        ],
        [
            GetResponse::ToolsChanged,
            GetResponse::SessionExpired,
            GetResponse::ToolsChanged,
        ],
    )
    .await?;
    harness.wait_for_gets(1).await;

    let (first, second, third) = tokio::join!(
        harness.ping(),
        async {
            tokio::time::sleep(Duration::from_millis(10)).await;
            harness.ping().await
        },
        async {
            tokio::time::sleep(Duration::from_millis(20)).await;
            harness.ping().await
        }
    );
    first.expect("the slow ping should succeed");
    let second_error = second.expect_err("the interrupted ping cannot be replayed");
    assert!(
        second_error.to_string().contains("Session expired"),
        "{second_error}"
    );
    third.expect("the queued ping should run on the replacement session");

    assert_eq!(
        harness.ping_sessions(),
        vec![
            Some("session-1".to_owned()),
            Some("session-1".to_owned()),
            Some("session-2".to_owned())
        ]
    );
    harness.finish().await
}

#[tokio::test]
async fn established_listener_session_expired_reinitializes_once() -> anyhow::Result<()> {
    let harness = Harness::start_with(
        Some("session-1"),
        Some("session-2"),
        true,
        [PingResponse::Json],
        [
            GetResponse::EmptySse,
            GetResponse::SessionExpired,
            GetResponse::SessionExpired,
        ],
    )
    .await?;

    harness.wait_for_gets(3).await;
    // The replacement listener's own 404 must not start another initialize
    // within one session_recovery_timeout.
    harness.assert_get_count_stays(3).await;
    assert_eq!(harness.state.initialize_calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        harness.get_sessions(),
        vec![
            Some("session-1".to_owned()),
            Some("session-1".to_owned()),
            Some("session-2".to_owned())
        ]
    );
    harness.ping().await?;
    harness.finish().await
}
