//! SEP-2260 follow-up (#1033): stream-based receive-side enforcement.
//!
//! Scripted streamable HTTP "server": answers a legacy initialize with
//! protocol 2026-07-28 AND a session id. That combination is NOT
//! spec-compliant: the 2026-07-28 revision removes protocol-level sessions
//! and the standalone GET stream (SEP-2567; transports spec: "do not mint
//! or echo session IDs"). Receive-side enforcement (#1033) exists precisely
//! to protect the client from non-conforming servers, and rmcp's client
//! tolerates the session id and opens the standalone GET stream — so this
//! is the reachable path where the client has BOTH a GET stream and strict
//! SEP-2260 enforcement.
#![cfg(all(
    feature = "client",
    feature = "transport-streamable-http-client",
    not(feature = "local")
))]
#![expect(
    deprecated,
    reason = "Sampling is deprecated by SEP-2577 but remains the canonical restricted request"
)]

use std::{collections::HashMap, sync::Arc, time::Duration};

use futures::{StreamExt, stream::BoxStream};
use http::{HeaderName, HeaderValue};
use rmcp::{
    ClientHandler,
    model::{
        ClientJsonRpcMessage, CreateMessageRequestParams, CreateMessageResult,
        InitializeRequestParams, InitializeResult, ProtocolVersion, SamplingMessage,
        ServerCapabilities, ServerJsonRpcMessage,
    },
    service::{ClientLifecycleMode, RequestContext, RoleClient, serve_client_with_lifecycle},
    transport::streamable_http_client::{
        StreamableHttpClient, StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
        StreamableHttpError, StreamableHttpPostResponse,
    },
};
use serde_json::{Value, json};
use sse_stream::{Error as SseError, Sse};
use tokio::sync::{Mutex, mpsc};

fn to_sse(message: Value) -> Result<Sse, SseError> {
    Ok(Sse {
        event: None,
        data: Some(message.to_string()),
        id: None,
        retry: None,
    })
}

fn message_stream(rx: mpsc::Receiver<Value>) -> BoxStream<'static, Result<Sse, SseError>> {
    tokio_stream::wrappers::ReceiverStream::new(rx)
        .map(to_sse)
        .boxed()
}

/// Scripted server: initialize -> JSON init result (2026-07-28 + session);
/// first non-initialize request POST -> SSE stream fed by `post_stream`;
/// everything else -> Accepted. Every message the client POSTs is forwarded
/// to `posted`.
#[derive(Clone)]
struct ScriptedServer {
    get_stream: Arc<Mutex<Option<mpsc::Receiver<Value>>>>,
    post_stream: Arc<Mutex<Option<mpsc::Receiver<Value>>>>,
    posted: mpsc::UnboundedSender<Value>,
}

impl StreamableHttpClient for ScriptedServer {
    type Error = std::io::Error;

    async fn post_message(
        &self,
        _uri: Arc<str>,
        message: ClientJsonRpcMessage,
        _session_id: Option<Arc<str>>,
        _auth_header: Option<String>,
        _custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>> {
        let value = serde_json::to_value(&message).expect("serialize client message");
        // Receiver drop is normal at test teardown; never panic in the transport task.
        let _ = self.posted.send(value.clone());
        if value["method"] == "initialize" {
            let mut info = InitializeResult::new(ServerCapabilities::default());
            info.protocol_version = ProtocolVersion::V_2026_07_28;
            let response = ServerJsonRpcMessage::response(
                rmcp::model::ServerResult::InitializeResult(info),
                serde_json::from_value(value["id"].clone()).expect("request id"),
            );
            return Ok(StreamableHttpPostResponse::Json(
                response,
                Some("scripted-session".into()),
            ));
        }
        if matches!(message, ClientJsonRpcMessage::Request(_)) {
            // Fail as a transport error rather than panicking: this code runs
            // in the transport task, where a panic is swallowed and shows up
            // only as an opaque timeout in the test.
            let rx = self.post_stream.lock().await.take().ok_or_else(|| {
                StreamableHttpError::Client(std::io::Error::other(
                    "scripted server expects exactly one non-initialize request POST",
                ))
            })?;
            return Ok(StreamableHttpPostResponse::Sse(message_stream(rx), None));
        }
        Ok(StreamableHttpPostResponse::Accepted)
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
    ) -> Result<BoxStream<'static, Result<Sse, SseError>>, StreamableHttpError<Self::Error>> {
        match self.get_stream.lock().await.take() {
            Some(rx) => Ok(message_stream(rx)),
            // Reconnect after the scripted stream ends: stay silent.
            None => Ok(futures::stream::pending().boxed()),
        }
    }
}

#[derive(Clone)]
struct SamplingClient {
    // Every invocation is recorded here. Tests assert on the receiver (the
    // negative test asserts it stays empty); a panic in this handler would
    // run in a spawned task and be silently swallowed.
    sampled: mpsc::UnboundedSender<CreateMessageRequestParams>,
}

impl ClientHandler for SamplingClient {
    async fn create_message(
        &self,
        params: CreateMessageRequestParams,
        _context: RequestContext<RoleClient>,
    ) -> Result<CreateMessageResult, rmcp::ErrorData> {
        let _ = self.sampled.send(params);
        Ok(CreateMessageResult::new(
            SamplingMessage::assistant_text("pong"),
            "test-model".to_string(),
        ))
    }

    fn get_info(&self) -> InitializeRequestParams {
        InitializeRequestParams::default()
    }
}

fn sampling_request(id: u32) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "sampling/createMessage",
        "params": {
            "messages": [{ "role": "user", "content": { "type": "text", "text": "hi" } }],
            "maxTokens": 16
        }
    })
}

fn tools_list_response(id: &Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": { "tools": [] } })
}

async fn next_posted(posted: &mut mpsc::UnboundedReceiver<Value>) -> Value {
    tokio::time::timeout(Duration::from_secs(5), posted.recv())
        .await
        .expect("posted message within 5s")
        .expect("channel open")
}

struct Harness {
    client: rmcp::service::RunningService<RoleClient, SamplingClient>,
    /// Every message the client POSTs to the scripted server.
    posted: mpsc::UnboundedReceiver<Value>,
    /// Feeds the standalone GET stream.
    get_tx: mpsc::Sender<Value>,
    /// Feeds the SSE stream of the in-flight tools/list POST.
    post_tx: mpsc::Sender<Value>,
    /// In-flight tools/list call (response withheld until the test releases it).
    call: tokio::task::JoinHandle<Result<rmcp::model::ListToolsResult, rmcp::ServiceError>>,
    tools_list_id: Value,
    /// Sampling params seen by the client handler.
    sampled: mpsc::UnboundedReceiver<CreateMessageRequestParams>,
}

/// Drive startup + one in-flight tools/list.
async fn setup() -> Harness {
    let (get_tx, get_rx) = mpsc::channel(8);
    let (post_tx, post_rx) = mpsc::channel(8);
    let (posted_tx, mut posted_rx) = mpsc::unbounded_channel();
    let (sampled_tx, sampled_rx) = mpsc::unbounded_channel();
    let server = ScriptedServer {
        get_stream: Arc::new(Mutex::new(Some(get_rx))),
        post_stream: Arc::new(Mutex::new(Some(post_rx))),
        posted: posted_tx,
    };
    let transport = StreamableHttpClientTransport::with_client(
        server,
        StreamableHttpClientTransportConfig::with_uri("http://scripted/mcp"),
    );
    let client = serve_client_with_lifecycle(
        SamplingClient {
            sampled: sampled_tx,
        },
        transport,
        ClientLifecycleMode::Initialize,
    )
    .await
    .expect("initialize against scripted server");

    // initialize + notifications/initialized already posted during startup.
    assert_eq!(next_posted(&mut posted_rx).await["method"], "initialize");
    assert_eq!(
        next_posted(&mut posted_rx).await["method"],
        "notifications/initialized"
    );

    // Unrelated outbound request, kept in flight (response withheld).
    let peer = client.peer().clone();
    let call = tokio::spawn(async move { peer.list_tools(None).await });
    let tools_list = next_posted(&mut posted_rx).await;
    assert_eq!(tools_list["method"], "tools/list");
    let tools_list_id = tools_list["id"].clone();

    Harness {
        client,
        posted: posted_rx,
        get_tx,
        post_tx,
        call,
        tools_list_id,
        sampled: sampled_rx,
    }
}

/// #1033 scenario 1: a restricted request on the standalone GET stream while
/// an unrelated outbound request is in flight must be rejected with -32602.
/// (The coarse check from #1029 incorrectly accepted this.)
#[tokio::test]
async fn restricted_request_on_get_stream_rejected_while_unrelated_request_in_flight()
-> anyhow::Result<()> {
    let mut h = setup().await;

    h.get_tx.send(sampling_request(100)).await?;

    let rejection = next_posted(&mut h.posted).await;
    assert_eq!(
        rejection["id"], 100,
        "reply to the sampling request: {rejection}"
    );
    assert_eq!(
        rejection["error"]["code"], -32602,
        "SEP-2260: GET-stream request must be rejected even with an unrelated \
         request in flight, got {rejection}"
    );

    h.post_tx
        .send(tools_list_response(&h.tools_list_id))
        .await?;
    tokio::time::timeout(Duration::from_secs(5), h.call).await???;
    // Rejected means rejected: the handler must not ALSO have been invoked.
    assert!(
        h.sampled.try_recv().is_err(),
        "handler must not see the rejected sampling request"
    );
    tokio::time::timeout(Duration::from_secs(5), h.client.cancel()).await??;
    Ok(())
}

/// Positive twin: the same restricted request arriving on the SSE stream of
/// the originating POST is dispatched to the handler and answered.
#[tokio::test]
async fn restricted_request_on_originating_post_stream_is_dispatched() -> anyhow::Result<()> {
    let mut h = setup().await;

    h.post_tx.send(sampling_request(200)).await?;

    let response = next_posted(&mut h.posted).await;
    assert_eq!(
        response["id"], 200,
        "reply to the sampling request: {response}"
    );
    assert_eq!(
        response["result"]["model"], "test-model",
        "request on the originating POST stream must reach the handler, got {response}"
    );
    tokio::time::timeout(Duration::from_secs(5), h.sampled.recv())
        .await?
        .expect("handler invoked");

    h.post_tx
        .send(tools_list_response(&h.tools_list_id))
        .await?;
    tokio::time::timeout(Duration::from_secs(5), h.call).await???;
    tokio::time::timeout(Duration::from_secs(5), h.client.cancel()).await??;
    Ok(())
}
