//! Independent http POSTs may overlap. A POST that reports an expired session
//! is retried at most once.
#![cfg(not(feature = "local"))]

use std::{
    collections::HashMap,
    io,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst},
    },
    time::Duration,
};

use futures::{StreamExt, stream::BoxStream};
use http::{HeaderName, HeaderValue};
use rmcp::{
    model::{
        CallToolRequestParams, CancelledNotificationParam, ClientInfo, ClientJsonRpcMessage,
        ClientRequest, DiscoverResult, ProtocolVersion, Request, RequestId, RequestMetaObject,
        ServerJsonRpcMessage,
    },
    service::{
        ClientLifecycleMode, PeerRequestOptions, RequestHandle, RoleClient, RunningService,
        serve_client_with_lifecycle,
    },
    transport::streamable_http_client::{
        StreamableHttpClient, StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
        StreamableHttpError, StreamableHttpPostResponse,
    },
};
use serde_json::{Value, json};
use sse_stream::{Error as SseError, Sse};
use tokio::{
    sync::{Mutex, mpsc, oneshot},
    task::JoinHandle,
    time::timeout,
};
use tokio_stream::wrappers::UnboundedReceiverStream;

const TEST_TIMEOUT: Duration = Duration::from_secs(5);
type PostResult = Result<StreamableHttpPostResponse, StreamableHttpError<io::Error>>;
type Call = JoinHandle<anyhow::Result<()>>;
type SseReceiver = mpsc::UnboundedReceiver<Result<Sse, SseError>>;

#[derive(Default)]
struct Counts {
    initialized: AtomicUsize,
    hold_reinitialization: AtomicBool,
    manual_controls: AtomicBool,
    deleted: AtomicUsize,
    cancelled: AtomicUsize,
    posted: AtomicUsize,
    active: AtomicUsize,
    peak: AtomicUsize,
}

struct ActivePost(Arc<Counts>);

impl Drop for ActivePost {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, SeqCst);
    }
}

struct Posted {
    id: Value,
    name: String,
    session: Option<Arc<str>>,
    reply: oneshot::Sender<PostResult>,
    returned: oneshot::Receiver<()>,
}

struct ControlPost {
    message: Value,
    session: Option<Arc<str>>,
    reply: oneshot::Sender<PostResult>,
}

fn response(id: Value, result: Value) -> ServerJsonRpcMessage {
    serde_json::from_value(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
        .expect("valid scripted response")
}

fn sse(message: Value) -> Result<Sse, SseError> {
    Ok(Sse {
        event: Some("message".into()),
        data: Some(message.to_string()),
        id: None,
        retry: None,
    })
}

async fn next_event<T>(receiver: &mut mpsc::UnboundedReceiver<T>) -> T {
    timeout(TEST_TIMEOUT, receiver.recv())
        .await
        .expect("expected scripted event")
        .expect("scripted client remains connected")
}

impl Posted {
    fn result(&self) -> ServerJsonRpcMessage {
        response(
            self.id.clone(),
            json!({ "content": [{ "type": "text", "text": self.name }] }),
        )
    }

    fn finish(self, result: PostResult) {
        self.reply.send(result).expect("POST is still waiting");
    }

    fn succeed(self) {
        let result = StreamableHttpPostResponse::Json(self.result(), None);
        self.finish(Ok(result));
    }

    fn expire(self) {
        self.finish(Err(StreamableHttpError::SessionExpired));
    }

    async fn finish_and_wait(self, result: PostResult) -> anyhow::Result<()> {
        let Self {
            reply, returned, ..
        } = self;
        reply.send(result).expect("POST is still waiting");
        timeout(TEST_TIMEOUT, returned).await??;
        Ok(())
    }

    async fn expire_and_wait(self) -> anyhow::Result<()> {
        self.finish_and_wait(Err(StreamableHttpError::SessionExpired))
            .await
    }

    async fn start_sse(self) -> anyhow::Result<oneshot::Sender<()>> {
        let message = serde_json::to_value(self.result()).unwrap();
        let (release, released) = oneshot::channel();
        let stream = futures::stream::once(async move {
            released.await.expect("release the SSE response");
            sse(message)
        })
        .boxed();
        self.finish_and_wait(Ok(StreamableHttpPostResponse::Sse(stream, None)))
            .await?;
        Ok(release)
    }
}

#[derive(Clone)]
struct ScriptedClient {
    started: mpsc::UnboundedSender<Posted>,
    controls: mpsc::UnboundedSender<ControlPost>,
    incoming: Arc<Mutex<Option<SseReceiver>>>,
    reinitializing: mpsc::UnboundedSender<oneshot::Sender<()>>,
    counts: Arc<Counts>,
}

impl ScriptedClient {
    async fn control_post(&self, message: Value, session: Option<Arc<str>>) -> PostResult {
        if !self.counts.manual_controls.load(SeqCst) {
            return Ok(StreamableHttpPostResponse::Accepted);
        }
        let (reply, result) = oneshot::channel();
        self.controls
            .send(ControlPost {
                message,
                session,
                reply,
            })
            .expect("test remains connected");
        result.await.expect("test answers the control POST")
    }
}

impl StreamableHttpClient for ScriptedClient {
    type Error = io::Error;

    async fn post_message(
        &self,
        _uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session: Option<Arc<str>>,
        _auth_header: Option<String>,
        _custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> PostResult {
        let value = serde_json::to_value(message).unwrap();
        match value["method"].as_str() {
            Some("server/discover") => Ok(StreamableHttpPostResponse::Json(
                response(
                    value["id"].clone(),
                    serde_json::to_value(DiscoverResult::new(
                        vec![ProtocolVersion::V_2026_07_28],
                        serde_json::from_value(json!({ "tools": {} })).unwrap(),
                    ))
                    .unwrap(),
                ),
                None,
            )),
            Some("initialize") => {
                let generation = self.counts.initialized.fetch_add(1, SeqCst) + 1;
                if generation > 1 && self.counts.hold_reinitialization.load(SeqCst) {
                    let (release, released) = oneshot::channel();
                    self.reinitializing
                        .send(release)
                        .expect("test remains connected");
                    released.await.expect("test releases reinitialization");
                }
                Ok(StreamableHttpPostResponse::Json(
                    response(
                        value["id"].clone(),
                        json!({
                            "protocolVersion": "2025-11-25",
                            "capabilities": { "tools": {} },
                            "serverInfo": { "name": "scripted", "version": "1" },
                        }),
                    ),
                    Some(format!("session-{generation}")),
                ))
            }
            Some("notifications/initialized") => Ok(StreamableHttpPostResponse::Accepted),
            Some("notifications/cancelled") => {
                self.counts.cancelled.fetch_add(1, SeqCst);
                self.control_post(value, session).await
            }
            Some("tools/call") => {
                self.counts.posted.fetch_add(1, SeqCst);
                let active = self.counts.active.fetch_add(1, SeqCst) + 1;
                self.counts.peak.fetch_max(active, SeqCst);
                let _active = ActivePost(self.counts.clone());
                let (reply, response) = oneshot::channel();
                let (finished, returned) = oneshot::channel();
                self.started
                    .send(Posted {
                        id: value["id"].clone(),
                        name: value["params"]["name"].as_str().unwrap().to_owned(),
                        session,
                        reply,
                        returned,
                    })
                    .expect("test remains connected");
                let response = response.await.expect("test answers each POST");
                let _ = finished.send(());
                response
            }
            None if value.get("result").is_some() || value.get("error").is_some() => {
                self.control_post(value, session).await
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
        assert_eq!(
            self.counts.active.load(SeqCst),
            0,
            "POSTs must stop before deleting the session"
        );
        self.counts.deleted.fetch_add(1, SeqCst);
        Ok(())
    }

    async fn get_stream(
        &self,
        _uri: Arc<str>,
        _session: Option<Arc<str>>,
        _last_event_id: Option<String>,
        _auth_header: Option<String>,
        _custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<BoxStream<'static, Result<Sse, SseError>>, StreamableHttpError<Self::Error>> {
        Ok(match self.incoming.lock().await.take() {
            Some(incoming) => UnboundedReceiverStream::new(incoming).boxed(),
            None => futures::stream::pending().boxed(),
        })
    }
}

struct Harness {
    client: RunningService<RoleClient, ClientInfo>,
    started: mpsc::UnboundedReceiver<Posted>,
    controls: mpsc::UnboundedReceiver<ControlPost>,
    incoming: mpsc::UnboundedSender<Result<Sse, SseError>>,
    reinitializations: mpsc::UnboundedReceiver<oneshot::Sender<()>>,
    counts: Arc<Counts>,
}

fn config() -> StreamableHttpClientTransportConfig {
    StreamableHttpClientTransportConfig::with_uri("http://scripted/mcp")
}

fn transport_error(error: &anyhow::Error) -> &StreamableHttpError<io::Error> {
    let service_error = error
        .downcast_ref::<rmcp::ServiceError>()
        .expect("expected a service error");
    let rmcp::ServiceError::TransportSend(transport_error) = service_error else {
        panic!("expected a transport error, got {service_error:?}");
    };
    transport_error
        .error
        .downcast_ref::<StreamableHttpError<io::Error>>()
        .expect("expected a streamable http error")
}

fn assert_recovery_timeout(error: anyhow::Error) {
    assert!(matches!(
        transport_error(&error),
        StreamableHttpError::SessionRecoveryTimeout
    ));
}

impl Harness {
    async fn start(config: StreamableHttpClientTransportConfig) -> anyhow::Result<Self> {
        Self::with_lifecycle(config, ClientLifecycleMode::Initialize).await
    }

    async fn with_lifecycle(
        config: StreamableHttpClientTransportConfig,
        lifecycle: ClientLifecycleMode,
    ) -> anyhow::Result<Self> {
        let (started, requests) = mpsc::unbounded_channel();
        let (control_tx, controls) = mpsc::unbounded_channel();
        let (incoming, incoming_rx) = mpsc::unbounded_channel();
        let (reinitializing, reinitializations) = mpsc::unbounded_channel();
        let counts = Arc::new(Counts::default());
        let transport = StreamableHttpClientTransport::with_client(
            ScriptedClient {
                started,
                controls: control_tx,
                incoming: Arc::new(Mutex::new(Some(incoming_rx))),
                reinitializing,
                counts: counts.clone(),
            },
            config,
        );
        let client =
            serve_client_with_lifecycle(ClientInfo::default(), transport, lifecycle).await?;
        Ok(Self {
            client,
            started: requests,
            controls,
            incoming,
            reinitializations,
            counts,
        })
    }

    fn call(&self, name: impl Into<String>) -> Call {
        let name = name.into();
        let peer = self.client.peer().clone();
        tokio::spawn(async move {
            let result = peer
                .call_tool(CallToolRequestParams::new(name.clone()))
                .await?;
            anyhow::ensure!(serde_json::to_value(result)?["content"][0]["text"] == name);
            Ok(())
        })
    }

    async fn cancellable(&self, name: &'static str) -> anyhow::Result<RequestHandle<RoleClient>> {
        self.cancellable_with_options(name, PeerRequestOptions::no_options())
            .await
    }

    async fn cancellable_with_options(
        &self,
        name: &'static str,
        options: PeerRequestOptions,
    ) -> anyhow::Result<RequestHandle<RoleClient>> {
        Ok(self
            .client
            .peer()
            .send_cancellable_request(
                ClientRequest::CallToolRequest(Request::new(CallToolRequestParams::new(name))),
                options,
            )
            .await?)
    }

    fn notify_cancellation(&self, id: RequestId) -> JoinHandle<Result<(), rmcp::ServiceError>> {
        let peer = self.client.peer().clone();
        tokio::spawn(async move {
            peer.notify_cancelled(CancelledNotificationParam::new(Some(id), None))
                .await
        })
    }

    async fn next(&mut self) -> Posted {
        next_event(&mut self.started).await
    }

    async fn next_control(&mut self) -> ControlPost {
        next_event(&mut self.controls).await
    }

    async fn exchange_ping(&mut self, id: &str) {
        self.incoming
            .send(sse(json!({ "jsonrpc": "2.0", "id": id, "method": "ping" })))
            .expect("common SSE stream remains open");
        let control = self.next_control().await;
        assert_eq!(control.message["id"], id);
        assert!(control.message["result"].is_object());
        assert_eq!(control.session.as_deref(), Some("session-1"));
        control
            .reply
            .send(Ok(StreamableHttpPostResponse::Accepted))
            .expect("reply POST is still waiting");
    }

    async fn finish(
        self,
        calls: Vec<Call>,
        posted: usize,
        initialized: usize,
    ) -> anyhow::Result<()> {
        for call in calls {
            timeout(TEST_TIMEOUT, call).await???;
        }
        assert_eq!(self.counts.posted.load(SeqCst), posted);
        assert_eq!(self.counts.initialized.load(SeqCst), initialized);
        assert_eq!(self.counts.active.load(SeqCst), 0);
        self.client.cancel().await?;
        Ok(())
    }
}

#[tokio::test]
async fn json_limits_allow_overlap_and_preserve_response_ids() -> anyhow::Result<()> {
    let mut zero = config();
    zero.max_concurrent_requests = 0;
    for (config, limit, total) in [
        (config().max_concurrent_requests(2), 2, 5),
        (config().max_concurrent_requests(1), 1, 3),
        (zero, 1, 2),
        (config(), 16, 17),
    ] {
        let mut harness = Harness::start(config).await?;
        let calls = (0..total)
            .map(|index| harness.call(format!("request-{index}")))
            .collect();
        let mut pending = Vec::new();
        for _ in 0..limit {
            pending.push(harness.next().await);
        }
        assert_eq!(harness.counts.active.load(SeqCst), limit);
        // Keep the oldest response blocked while newer requests finish first.
        for _ in limit..total {
            pending.pop().unwrap().succeed();
            pending.push(harness.next().await);
        }
        for request in pending.into_iter().rev() {
            request.succeed();
        }
        let counts = harness.counts.clone();
        harness.finish(calls, total, 1).await?;
        assert_eq!(counts.peak.load(SeqCst), limit);
    }
    Ok(())
}

#[tokio::test]
async fn early_sse_response_releases_the_post_slot() -> anyhow::Result<()> {
    let mut harness = Harness::start(config().max_concurrent_requests(1)).await?;
    let first = harness.call("first");
    let release = harness.next().await.start_sse().await?;
    let second = harness.call("second");
    harness.next().await.succeed();
    timeout(TEST_TIMEOUT, second).await???;
    assert!(!first.is_finished(), "the SSE response is still blocked");
    release.send(()).unwrap();
    assert_eq!(harness.counts.peak.load(SeqCst), 1);
    harness.finish(vec![first], 2, 1).await
}

#[tokio::test]
async fn concurrent_session_expiry_shares_one_reinitialization() -> anyhow::Result<()> {
    let mut harness = Harness::start(config().max_concurrent_requests(2)).await?;
    let calls = vec![harness.call("first"), harness.call("second")];
    // Hold both requests before releasing either expired response.
    let first = harness.next().await;
    let second = harness.next().await;
    let mut originals = HashMap::new();
    for request in [first, second] {
        assert_eq!(request.session.as_deref(), Some("session-1"));
        originals.insert(request.name.clone(), request.id.clone());
        request.expire();
    }
    for _ in 0..2 {
        let retry = harness.next().await;
        assert_eq!(retry.session.as_deref(), Some("session-2"));
        assert_eq!(originals.remove(&retry.name), Some(retry.id.clone()));
        retry.succeed();
    }
    assert!(originals.is_empty());
    harness.finish(calls, 4, 2).await
}

#[tokio::test]
async fn cancellation_still_runs_while_recovery_waits_for_old_posts() -> anyhow::Result<()> {
    let mut harness = Harness::start(config().max_concurrent_requests(3)).await?;
    let hanging = harness.cancellable("hanging").await?;
    let mut blocked = harness.next().await;
    let expired = harness.call("expired");
    harness.next().await.expire_and_wait().await?;
    assert_eq!(harness.counts.initialized.load(SeqCst), 1);
    assert_eq!(harness.counts.active.load(SeqCst), 1);

    timeout(TEST_TIMEOUT, hanging.cancel(None))
        .await
        .expect("cancellation must bypass the session recovery wait")?;
    timeout(TEST_TIMEOUT, blocked.reply.closed()).await?;
    let retry = harness.next().await;
    assert_eq!(retry.name, "expired");
    assert_eq!(retry.session.as_deref(), Some("session-2"));
    retry.succeed();
    harness.finish(vec![expired], 3, 2).await
}

#[tokio::test]
async fn server_replies_still_run_while_recovery_waits_for_old_posts() -> anyhow::Result<()> {
    let mut harness = Harness::start(config().max_concurrent_requests(2)).await?;
    harness.counts.manual_controls.store(true, SeqCst);
    let waiting = harness.call("waiting-for-client");
    let blocked = harness.next().await;
    let expired = harness.call("expired");
    harness.next().await.expire_and_wait().await?;

    harness.exchange_ping("recovery-ping").await;
    blocked.succeed();
    let retry = harness.next().await;
    assert_eq!(retry.name, "expired");
    assert_eq!(retry.session.as_deref(), Some("session-2"));
    retry.succeed();
    harness.finish(vec![waiting, expired], 3, 2).await
}

#[tokio::test]
async fn a_version_barrier_allows_the_server_reply_it_needs() -> anyhow::Result<()> {
    let mut harness = Harness::start(config().max_concurrent_requests(2)).await?;
    harness.counts.manual_controls.store(true, SeqCst);
    let mut meta = RequestMetaObject::new();
    meta.set_protocol_version(ProtocolVersion::V_2025_06_18);
    let barrier = harness
        .cancellable_with_options("barrier", PeerRequestOptions::no_options().with_meta(meta))
        .await?;
    let blocked = harness.next().await;
    let queued = harness.cancellable("after-barrier").await?;

    harness.exchange_ping("barrier-ping").await;
    assert_eq!(harness.counts.posted.load(SeqCst), 1);
    blocked.succeed();
    timeout(TEST_TIMEOUT, barrier.await_response()).await??;
    let next = harness.next().await;
    assert_eq!(next.name, "after-barrier");
    next.succeed();
    timeout(TEST_TIMEOUT, queued.await_response()).await??;
    harness.finish(vec![], 2, 1).await
}

#[tokio::test]
async fn recovery_deadline_drops_ambiguous_posts_without_retrying_them() -> anyhow::Result<()> {
    assert_eq!(config().session_recovery_timeout, Duration::from_secs(5));
    let mut harness = Harness::start(
        config()
            .max_concurrent_requests(3)
            .session_recovery_timeout(Duration::from_millis(50)),
    )
    .await?;
    let ambiguous = harness.call("possibly-applied");
    let mut blocked = harness.next().await;
    let expired = harness.call("expired");
    let rejected = harness.next().await;
    let rejected_id = rejected.id.clone();
    rejected.expire_and_wait().await?;

    assert_recovery_timeout(timeout(TEST_TIMEOUT, ambiguous).await??.unwrap_err());
    timeout(TEST_TIMEOUT, blocked.reply.closed()).await?;
    let retry = harness.next().await;
    assert_eq!(retry.name, "expired");
    assert_eq!(retry.id, rejected_id);
    assert_eq!(retry.session.as_deref(), Some("session-2"));
    retry.succeed();
    harness.finish(vec![expired], 3, 2).await
}

#[tokio::test]
async fn reinitialization_has_its_own_deadline() -> anyhow::Result<()> {
    let mut harness =
        Harness::start(config().session_recovery_timeout(Duration::from_millis(50))).await?;
    harness.counts.hold_reinitialization.store(true, SeqCst);
    let expired = harness.call("expired");
    harness.next().await.expire_and_wait().await?;
    let mut reinitialization = next_event(&mut harness.reinitializations).await;

    assert_recovery_timeout(timeout(TEST_TIMEOUT, expired).await??.unwrap_err());
    timeout(TEST_TIMEOUT, reinitialization.closed()).await?;
    harness.finish(vec![], 1, 2).await
}

#[tokio::test]
async fn an_expired_retry_is_not_retried_again() -> anyhow::Result<()> {
    let mut harness = Harness::start(config().max_concurrent_requests(2)).await?;
    let call = harness.call("expires-twice");
    let first = harness.next().await;
    let id = first.id.clone();
    first.expire();
    let retry = harness.next().await;
    assert_eq!(retry.id, id);
    assert_eq!(retry.session.as_deref(), Some("session-2"));
    retry.expire();
    let error = timeout(TEST_TIMEOUT, call).await??.unwrap_err();
    assert!(error.to_string().contains("Session expired"));
    harness.finish(vec![], 2, 2).await
}

#[tokio::test]
async fn a_lost_post_response_is_not_retried() -> anyhow::Result<()> {
    let mut harness = Harness::start(config().max_concurrent_requests(2)).await?;
    let call = harness.call("possibly-applied");
    harness
        .next()
        .await
        .finish(Err(StreamableHttpError::Client(io::Error::new(
            io::ErrorKind::ConnectionReset,
            "scripted response lost",
        ))));
    let error = timeout(TEST_TIMEOUT, call).await??.unwrap_err();
    assert!(error.to_string().contains("scripted response lost"));
    harness.finish(vec![], 1, 1).await
}

#[tokio::test]
async fn cancellation_bypasses_queued_posts_at_capacity() -> anyhow::Result<()> {
    for (lifecycle, legacy_notifications, initializations) in [
        (ClientLifecycleMode::Initialize, 2, 1),
        (
            ClientLifecycleMode::Discover {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            },
            0,
            0,
        ),
    ] {
        let mut harness =
            Harness::with_lifecycle(config().max_concurrent_requests(1), lifecycle).await?;
        let request = harness.cancellable("cancel-me").await?;
        let mut blocked = harness.next().await;
        let cancelled = harness.cancellable("never-send").await?;
        timeout(TEST_TIMEOUT, cancelled.cancel(None)).await??;
        let queued = harness.cancellable("queued").await?;
        timeout(TEST_TIMEOUT, request.cancel(None)).await??;
        timeout(TEST_TIMEOUT, blocked.reply.closed()).await?;
        let next = harness.next().await;
        assert_eq!(next.name, "queued");
        next.succeed();
        timeout(TEST_TIMEOUT, queued.await_response()).await??;
        assert_eq!(harness.counts.cancelled.load(SeqCst), legacy_notifications);
        harness.finish(vec![], 2, initializations).await?;
    }
    Ok(())
}

#[tokio::test]
async fn a_hanging_legacy_control_does_not_delay_local_cancellation() -> anyhow::Result<()> {
    let mut harness = Harness::start(config().max_concurrent_requests(1)).await?;
    harness.counts.manual_controls.store(true, SeqCst);
    let stale = harness.notify_cancellation(RequestId::Number(999));
    let mut held = harness.next_control().await;
    assert_eq!(held.message["params"]["requestId"], 999);
    harness.counts.manual_controls.store(false, SeqCst);

    let live = harness.cancellable("live-post").await?;
    let mut blocked = harness.next().await;
    let cancel_post = tokio::spawn(async move { live.cancel(None).await });
    timeout(Duration::from_secs(1), blocked.reply.closed()).await?;

    let streaming = harness.cancellable("live-stream").await?;
    let mut stream = harness.next().await.start_sse().await?;
    let cancel_stream = harness.notify_cancellation(streaming.id.clone());
    timeout(Duration::from_secs(1), stream.closed()).await?;
    assert!(
        !held.reply.is_closed(),
        "the old control POST is still held"
    );
    assert_eq!(harness.counts.cancelled.load(SeqCst), 1);

    // The private control timeout is five seconds; give its watchdog headroom.
    let error = timeout(Duration::from_secs(10), stale).await??.unwrap_err();
    assert!(matches!(
        transport_error(&error.into()),
        StreamableHttpError::ControlRequestTimeout
    ));
    timeout(TEST_TIMEOUT, held.reply.closed()).await?;
    timeout(TEST_TIMEOUT, cancel_post).await???;
    timeout(TEST_TIMEOUT, cancel_stream).await???;
    assert!(matches!(
        timeout(TEST_TIMEOUT, streaming.await_response()).await?,
        Err(rmcp::ServiceError::Cancelled { .. })
    ));
    assert_eq!(harness.counts.cancelled.load(SeqCst), 3);
    harness.finish(vec![], 2, 1).await
}

#[tokio::test]
async fn close_drops_blocked_posts_before_deleting_the_session() -> anyhow::Result<()> {
    let mut harness = Harness::start(config().max_concurrent_requests(2)).await?;
    let calls = [harness.call("first"), harness.call("second")];
    let posts = [harness.next().await, harness.next().await];
    timeout(TEST_TIMEOUT, harness.client.cancel()).await??;
    assert!(posts.iter().all(|post| post.reply.is_closed()));
    assert_eq!(harness.counts.active.load(SeqCst), 0);
    assert_eq!(harness.counts.deleted.load(SeqCst), 1);
    for call in calls {
        assert!(timeout(TEST_TIMEOUT, call).await??.is_err());
    }
    Ok(())
}
