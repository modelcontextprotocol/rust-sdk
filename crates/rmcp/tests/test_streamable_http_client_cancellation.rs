//! Transport cancellation drops active HTTP work before bounded session cleanup.
#![cfg(all(feature = "client", feature = "transport-streamable-http-client"))]

use std::{collections::HashMap, future::pending, io, sync::Arc, time::Duration};

use futures::{StreamExt, stream::BoxStream};
use http::{HeaderName, HeaderValue};
use rmcp::{
    model::{
        ClientJsonRpcMessage, InitializeResult, ProtocolVersion, RequestId, ServerCapabilities,
        ServerJsonRpcMessage, ServerResult,
    },
    transport::{
        Transport,
        streamable_http_client::{
            StreamableHttpClient, StreamableHttpClientTransport,
            StreamableHttpClientTransportConfig, StreamableHttpError, StreamableHttpPostResponse,
        },
    },
};
use serde_json::json;
use sse_stream::{Error as SseError, Sse};
use tokio::{
    sync::{mpsc, oneshot},
    time::timeout,
};
use tokio_util::sync::CancellationToken;

type PostResult = Result<StreamableHttpPostResponse, StreamableHttpError<io::Error>>;
type Post = (ClientJsonRpcMessage, oneshot::Sender<PostResult>);

#[derive(Debug)]
struct Delete {
    session: Arc<str>,
    auth: Option<String>,
    complete: oneshot::Sender<()>,
}

#[derive(Clone)]
struct Client {
    posts: mpsc::UnboundedSender<Post>,
    deletes: mpsc::UnboundedSender<Delete>,
    get_started: CancellationToken,
    get_dropped: CancellationToken,
}

impl StreamableHttpClient for Client {
    type Error = io::Error;

    async fn post_message(
        &self,
        _uri: Arc<str>,
        message: ClientJsonRpcMessage,
        _session: Option<Arc<str>>,
        _auth: Option<String>,
        _headers: HashMap<HeaderName, HeaderValue>,
    ) -> PostResult {
        let (reply, response) = oneshot::channel();
        self.posts.send((message, reply)).unwrap();
        response.await.unwrap()
    }

    async fn get_stream(
        &self,
        _uri: Arc<str>,
        _session: Option<Arc<str>>,
        _last_event_id: Option<String>,
        _auth: Option<String>,
        _headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<BoxStream<'static, Result<Sse, SseError>>, StreamableHttpError<Self::Error>> {
        let _dropped = self.get_dropped.clone().drop_guard();
        self.get_started.cancel();
        pending().await
    }

    async fn delete_session(
        &self,
        _uri: Arc<str>,
        session: Arc<str>,
        auth: Option<String>,
        _headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<(), StreamableHttpError<Self::Error>> {
        let (complete, response) = oneshot::channel();
        self.deletes
            .send(Delete {
                session,
                auth,
                complete,
            })
            .unwrap();
        response.await.unwrap();
        Ok(())
    }
}

struct Harness {
    transport: StreamableHttpClientTransport<Client>,
    posts: mpsc::UnboundedReceiver<Post>,
    deletes: mpsc::UnboundedReceiver<Delete>,
    get_started: CancellationToken,
    get_dropped: CancellationToken,
}

impl Harness {
    fn new() -> Self {
        let (posts, incoming) = mpsc::unbounded_channel();
        let (deletes, cleanup) = mpsc::unbounded_channel();
        let client = Client {
            posts,
            deletes,
            get_started: CancellationToken::new(),
            get_dropped: CancellationToken::new(),
        };
        Self {
            transport: StreamableHttpClientTransport::with_client(
                client.clone(),
                StreamableHttpClientTransportConfig::with_uri("http://scripted/mcp")
                    .auth_header("test-token"),
            ),
            posts: incoming,
            deletes: cleanup,
            get_started: client.get_started,
            get_dropped: client.get_dropped,
        }
    }

    async fn next_post(&mut self) -> Post {
        timeout(Duration::from_secs(1), self.posts.recv())
            .await
            .unwrap()
            .unwrap()
    }
}

fn initialize() -> ClientJsonRpcMessage {
    serde_json::from_value(json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": "2025-11-25", "capabilities": {},
            "clientInfo": {"name": "test", "version": "1"}}
    }))
    .unwrap()
}

#[rstest::rstest]
#[case::initialize_post(false, false)]
#[case::initialize_sse(false, true)]
#[case::fallback_post(true, false)]
#[case::fallback_sse(true, true)]
#[tokio::test]
async fn dropping_transport_cancels_initialization(
    #[case] fallback: bool,
    #[case] sse: bool,
) -> anyhow::Result<()> {
    let mut harness = Harness::new();
    if fallback {
        let discover = tokio::spawn(harness.transport.send(serde_json::from_value(json!({
            "jsonrpc": "2.0", "id": 0, "method": "server/discover"
        }))?));
        let (_, reply) = harness.next_post().await;
        reply
            .send(Ok(StreamableHttpPostResponse::Json(
                serde_json::from_value(json!({
                    "jsonrpc": "2.0", "id": 0,
                    "error": {"code": -32601, "message": "Method not found"}
                }))?,
                None,
            )))
            .unwrap();
        discover.await??;
        assert!(harness.transport.receive().await.is_some());
    }

    let send = tokio::spawn(harness.transport.send(initialize()));
    let (_, mut reply) = harness.next_post().await;
    if sse {
        let dropped = CancellationToken::new();
        let guard = dropped.clone().drop_guard();
        let stream = futures::stream::once(async move {
            let _guard = guard;
            pending().await
        })
        .boxed();
        reply
            .send(Ok(StreamableHttpPostResponse::Sse(stream, None)))
            .unwrap();
        send.await??;
        drop(harness.transport);
        timeout(Duration::from_secs(1), dropped.cancelled()).await?;
    } else {
        drop(harness.transport);
        timeout(Duration::from_secs(1), reply.closed()).await?;
        assert!(send.await?.is_err());
    }
    Ok(())
}

#[rstest::rstest]
#[case::initialized_post(false, false)]
#[case::get_headers(true, false)]
#[case::cleanup_timeout(false, true)]
#[tokio::test(start_paused = true)]
async fn close_preserves_session_cleanup(
    #[case] finish_initialized: bool,
    #[case] stall_delete: bool,
) -> anyhow::Result<()> {
    let mut harness = Harness::new();
    let send = tokio::spawn(harness.transport.send(initialize()));
    let (_, reply) = harness.next_post().await;
    reply
        .send(Ok(StreamableHttpPostResponse::Json(
            ServerJsonRpcMessage::response(
                ServerResult::InitializeResult(
                    InitializeResult::new(ServerCapabilities::default())
                        .with_protocol_version(ProtocolVersion::V_2025_11_25),
                ),
                RequestId::Number(1),
            ),
            Some("test-session".into()),
        )))
        .unwrap();
    send.await??;
    assert!(harness.transport.receive().await.is_some());

    let initialized = tokio::spawn(harness.transport.send(serde_json::from_value(json!({
        "jsonrpc": "2.0", "method": "notifications/initialized"
    }))?));
    let (_, reply) = harness.next_post().await;
    let pending_initialized = if finish_initialized {
        reply
            .send(Ok(StreamableHttpPostResponse::Accepted))
            .unwrap();
        timeout(Duration::from_secs(1), harness.get_started.cancelled()).await?;
        None
    } else {
        Some(reply)
    };

    let closed = harness.transport.cancel_token();
    let close = tokio::spawn(async move { harness.transport.close().await });
    let mut delete = timeout(Duration::from_secs(1), harness.deletes.recv())
        .await?
        .unwrap();
    assert!(closed.is_cancelled());
    assert_eq!(delete.session.as_ref(), "test-session");
    assert_eq!(delete.auth.as_deref(), Some("test-token"));
    assert!(!close.is_finished(), "close must wait for DELETE");
    if let Some(mut reply) = pending_initialized {
        assert!(initialized.await?.is_err());
        timeout(Duration::from_secs(1), reply.closed()).await?;
    } else {
        timeout(Duration::from_secs(1), harness.get_dropped.cancelled()).await?;
        initialized.await??;
    }
    if stall_delete {
        tokio::time::advance(Duration::from_secs(5)).await;
        timeout(Duration::from_secs(1), delete.complete.closed()).await?;
    } else {
        delete.complete.send(()).unwrap();
    }
    timeout(Duration::from_secs(1), close).await???;
    Ok(())
}
