//! Independent http POSTs may overlap. A POST that reports an expired session
//! is retried at most once.
#![cfg(not(feature = "local"))]

use std::{
    collections::HashMap,
    io,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering::SeqCst},
    },
    time::Duration,
};

use futures::{StreamExt, stream::BoxStream};
use http::{HeaderName, HeaderValue};
use rmcp::{
    model::{
        CallToolRequestParams, ClientInfo, ClientJsonRpcMessage, ClientRequest, DiscoverResult,
        ProtocolVersion, Request, ServerJsonRpcMessage,
    },
    service::{
        ClientLifecycleMode, PeerRequestOptions, RoleClient, RunningService,
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
    sync::{mpsc, oneshot},
    task::JoinHandle,
    time::timeout,
};

const TEST_TIMEOUT: Duration = Duration::from_secs(5);
type PostResult = Result<StreamableHttpPostResponse, StreamableHttpError<io::Error>>;
type Call = JoinHandle<anyhow::Result<()>>;

#[derive(Default)]
struct Counts {
    initialized: AtomicUsize,
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
}

fn response(id: Value, result: Value) -> ServerJsonRpcMessage {
    serde_json::from_value(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
        .expect("valid scripted response")
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

    fn start_sse(self) -> oneshot::Sender<()> {
        let data = serde_json::to_string(&self.result()).unwrap();
        let (release, released) = oneshot::channel();
        let stream = futures::stream::once(async move {
            released.await.expect("release the SSE response");
            Ok(Sse {
                event: Some("message".into()),
                data: Some(data),
                id: None,
                retry: None,
            })
        })
        .boxed();
        self.finish(Ok(StreamableHttpPostResponse::Sse(stream, None)));
        release
    }
}

#[derive(Clone)]
struct ScriptedClient {
    started: mpsc::UnboundedSender<Posted>,
    counts: Arc<Counts>,
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
                Ok(StreamableHttpPostResponse::Accepted)
            }
            Some("tools/call") => {
                self.counts.posted.fetch_add(1, SeqCst);
                let active = self.counts.active.fetch_add(1, SeqCst) + 1;
                self.counts.peak.fetch_max(active, SeqCst);
                let _active = ActivePost(self.counts.clone());
                let (reply, response) = oneshot::channel();
                self.started
                    .send(Posted {
                        id: value["id"].clone(),
                        name: value["params"]["name"].as_str().unwrap().to_owned(),
                        session,
                        reply,
                    })
                    .expect("test remains connected");
                response.await.expect("test answers each POST")
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
        Ok(futures::stream::pending().boxed())
    }
}

struct Harness {
    client: RunningService<RoleClient, ClientInfo>,
    started: mpsc::UnboundedReceiver<Posted>,
    counts: Arc<Counts>,
}

fn config() -> StreamableHttpClientTransportConfig {
    StreamableHttpClientTransportConfig::with_uri("http://scripted/mcp")
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
        let counts = Arc::new(Counts::default());
        let transport = StreamableHttpClientTransport::with_client(
            ScriptedClient {
                started,
                counts: counts.clone(),
            },
            config,
        );
        let client =
            serve_client_with_lifecycle(ClientInfo::default(), transport, lifecycle).await?;
        Ok(Self {
            client,
            started: requests,
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

    async fn next(&mut self) -> Posted {
        timeout(TEST_TIMEOUT, self.started.recv())
            .await
            .expect("expected POST to start")
            .expect("scripted client remains connected")
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
    let release = harness.next().await.start_sse();
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
async fn cancellation_drops_an_active_post() -> anyhow::Result<()> {
    for (lifecycle, legacy_notifications) in [
        (ClientLifecycleMode::Initialize, 1),
        (
            ClientLifecycleMode::Discover {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            },
            0,
        ),
    ] {
        let mut harness =
            Harness::with_lifecycle(config().max_concurrent_requests(2), lifecycle).await?;
        let request = harness
            .client
            .peer()
            .send_cancellable_request(
                ClientRequest::CallToolRequest(Request::new(CallToolRequestParams::new(
                    "cancel-me",
                ))),
                PeerRequestOptions::no_options(),
            )
            .await?;
        let mut blocked = harness.next().await;
        timeout(TEST_TIMEOUT, request.cancel(None)).await??;
        timeout(TEST_TIMEOUT, blocked.reply.closed()).await?;
        assert_eq!(harness.counts.cancelled.load(SeqCst), legacy_notifications);
        harness.finish(vec![], 1, legacy_notifications).await?;
    }
    Ok(())
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
