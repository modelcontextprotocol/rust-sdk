#![cfg(all(
    feature = "client",
    feature = "server",
    feature = "transport-streamable-http-client-reqwest",
    feature = "transport-streamable-http-server",
    not(feature = "local")
))]

use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use futures::StreamExt;
use rmcp::{
    ErrorData, ServerHandler,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock,
        ProgressNotificationParam, ServerCapabilities, ServerInfo,
    },
    service::RequestContext,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService,
        session::{
            EventStore, EventStoreError, EventStream, ServerSseMessage, SessionState, SessionStore,
            SessionStoreError, local::LocalSessionManager, never::NeverSessionManager,
        },
    },
};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Default)]
struct InMemorySessionStore(Arc<RwLock<HashMap<String, SessionState>>>);

#[async_trait::async_trait]
impl SessionStore for InMemorySessionStore {
    async fn load(&self, session_id: &str) -> Result<Option<SessionState>, SessionStoreError> {
        Ok(self.0.read().await.get(session_id).cloned())
    }

    async fn store(&self, session_id: &str, state: &SessionState) -> Result<(), SessionStoreError> {
        self.0
            .write()
            .await
            .insert(session_id.to_owned(), state.clone());
        Ok(())
    }

    async fn delete(&self, session_id: &str) -> Result<(), SessionStoreError> {
        self.0.write().await.remove(session_id);
        Ok(())
    }
}

#[derive(Clone)]
struct StoredEvent {
    stream_id: String,
    event: ServerSseMessage,
}

#[derive(Clone, Default)]
struct InMemoryEventStore {
    events: Arc<RwLock<Vec<StoredEvent>>>,
    next_id: Arc<AtomicU64>,
}

#[async_trait::async_trait]
impl EventStore for InMemoryEventStore {
    async fn store_event(
        &self,
        stream_id: &str,
        event: &ServerSseMessage,
    ) -> Result<String, EventStoreError> {
        let event_id = format!("event-{}", self.next_id.fetch_add(1, Ordering::Relaxed));
        let mut event = event.clone();
        event.event_id = Some(event_id.clone());
        self.events.write().await.push(StoredEvent {
            stream_id: stream_id.to_owned(),
            event,
        });
        Ok(event_id)
    }

    async fn replay_events_after(
        &self,
        last_event_id: &str,
    ) -> Result<EventStream, EventStoreError> {
        let events = self.events.read().await;
        let last_index = events
            .iter()
            .position(|stored| stored.event.event_id.as_deref() == Some(last_event_id))
            .ok_or_else(|| std::io::Error::other("event not found"))?;
        let stream_id = events[last_index].stream_id.clone();
        let replay = events
            .iter()
            .skip(last_index + 1)
            .filter(|stored| stored.stream_id == stream_id)
            .map(|stored| stored.event.clone())
            .collect::<Vec<_>>();
        Ok(Box::pin(futures::stream::iter(replay)))
    }
}

#[derive(Clone)]
struct ProgressServer;

impl ServerHandler for ProgressServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<rmcp::RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        if request.name == "progress" || request.name == "slow-progress" {
            let progress_token = context
                .meta
                .get_progress_token()
                .expect("request includes progressToken");
            context
                .peer
                .notify_progress(
                    ProgressNotificationParam::new(progress_token, 50.0)
                        .with_total(100.0)
                        .with_message("working"),
                )
                .await
                .expect("progress notification is delivered");
        }
        if request.name == "slow-progress" {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        Ok(CallToolResult::success(vec![ContentBlock::text("done")]).into())
    }
}

async fn spawn_server(
    session_store: Arc<dyn SessionStore>,
    event_store: Arc<dyn EventStore>,
    cancellation_token: &CancellationToken,
    legacy_session_mode: bool,
) -> anyhow::Result<(String, tokio::task::JoinHandle<()>)> {
    let config = {
        let mut config = StreamableHttpServerConfig::default();
        config.sse_keep_alive = None;
        config.legacy_session_mode = legacy_session_mode;
        config.cancellation_token = cancellation_token.child_token();
        config.session_store = Some(session_store);
        config
    };
    let router = if legacy_session_mode {
        let session_manager =
            Arc::new(LocalSessionManager::default().with_event_store(event_store));
        let service = StreamableHttpService::new(|| Ok(ProgressServer), session_manager, config);
        axum::Router::new().nest_service("/mcp", service)
    } else {
        let session_manager =
            Arc::new(NeverSessionManager::default().with_event_store(event_store));
        let service = StreamableHttpService::new(|| Ok(ProgressServer), session_manager, config);
        axum::Router::new().nest_service("/mcp", service)
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let handle = tokio::spawn({
        let cancellation_token = cancellation_token.clone();
        async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async move { cancellation_token.cancelled_owned().await })
                .await;
        }
    });
    Ok((format!("http://{address}/mcp"), handle))
}

fn event_id_containing<'a>(body: &'a str, needle: &str) -> Option<&'a str> {
    body.split("\n\n")
        .find(|event| event.contains(needle))?
        .lines()
        .find_map(|line| line.strip_prefix("id: "))
}

#[tokio::test]
async fn restored_instance_replays_events_from_shared_store() -> anyhow::Result<()> {
    let session_store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::default());
    let event_store: Arc<dyn EventStore> = Arc::new(InMemoryEventStore::default());
    let http = reqwest::Client::new();

    let cancellation_a = CancellationToken::new();
    let (url_a, server_a) = spawn_server(
        session_store.clone(),
        event_store.clone(),
        &cancellation_a,
        true,
    )
    .await?;
    let initialize = http
        .post(&url_a)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .body(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}"#)
        .send()
        .await?;
    let session_id = initialize
        .headers()
        .get("mcp-session-id")
        .expect("initialize returns a session ID")
        .to_str()?
        .to_owned();
    let _ = initialize.text().await?;

    let initialized_status = http
        .post(&url_a)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("Mcp-Session-Id", &session_id)
        .header("Mcp-Protocol-Version", "2025-06-18")
        .body(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
        .send()
        .await?
        .status();
    assert_eq!(initialized_status, reqwest::StatusCode::ACCEPTED);

    let original_body = http
        .post(&url_a)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("Mcp-Session-Id", &session_id)
        .header("Mcp-Protocol-Version", "2025-06-18")
        .body(r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"progress","arguments":{},"_meta":{"progressToken":"progress-1"}}}"#)
        .send()
        .await?
        .text()
        .await?;
    let progress_event_id = event_id_containing(&original_body, "notifications/progress")
        .expect("progress event has a persisted event ID")
        .to_owned();

    cancellation_a.cancel();
    server_a.await?;

    let cancellation_b = CancellationToken::new();
    let (url_b, server_b) = spawn_server(session_store, event_store, &cancellation_b, true).await?;
    let replay = http
        .get(&url_b)
        .header("Accept", "text/event-stream")
        .header("Mcp-Session-Id", &session_id)
        .header("Mcp-Protocol-Version", "2025-06-18")
        .header("Last-Event-ID", progress_event_id)
        .send()
        .await?;
    assert_eq!(replay.status(), reqwest::StatusCode::OK);
    let replay_body = replay.text().await?;
    assert!(
        replay_body.contains(r#""id":2"#),
        "instance B should replay the final response stored by instance A: {replay_body}"
    );

    cancellation_b.cancel();
    server_b.await?;
    Ok(())
}

#[tokio::test]
async fn stateless_instance_replays_events_from_shared_store() -> anyhow::Result<()> {
    let session_store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::default());
    let event_store = Arc::new(InMemoryEventStore::default());
    let http = reqwest::Client::new();

    let cancellation_a = CancellationToken::new();
    let (url_a, server_a) = spawn_server(
        session_store.clone(),
        event_store.clone(),
        &cancellation_a,
        false,
    )
    .await?;
    let original = http
        .post(&url_a)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "tools/call")
        .header("Mcp-Name", "slow-progress")
        .body(
            r#"{
                "jsonrpc":"2.0",
                "id":2,
                "method":"tools/call",
                "params":{
                    "name":"slow-progress",
                    "arguments":{},
                    "_meta":{
                        "progressToken":"progress-1",
                        "io.modelcontextprotocol/protocolVersion":"2026-07-28",
                        "io.modelcontextprotocol/clientInfo":{"name":"test","version":"1.0"},
                        "io.modelcontextprotocol/clientCapabilities":{}
                    }
                }
            }"#,
        )
        .send()
        .await?;
    assert_eq!(original.status(), reqwest::StatusCode::OK);
    assert!(
        original.headers().get("mcp-session-id").is_none(),
        "stateless response must not create a session"
    );
    let mut body = original.bytes_stream();
    let mut received = String::new();
    let progress_event_id = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let chunk = body
                .next()
                .await
                .expect("response remains open until progress arrives")?;
            received.push_str(&String::from_utf8_lossy(&chunk));
            if let Some(event_id) = event_id_containing(&received, "notifications/progress") {
                return Ok::<_, reqwest::Error>(event_id.to_owned());
            }
        }
    })
    .await??;
    drop(body);

    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let stored_response = event_store.events.read().await.iter().any(|stored| {
                stored.event.message.as_ref().is_some_and(|message| {
                    matches!(
                        message.as_ref(),
                        rmcp::model::ServerJsonRpcMessage::Response(_)
                    )
                })
            });
            if stored_response {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await?;

    cancellation_a.cancel();
    server_a.await?;

    let cancellation_b = CancellationToken::new();
    let (url_b, server_b) =
        spawn_server(session_store, event_store, &cancellation_b, false).await?;
    let replay = http
        .get(&url_b)
        .header("Accept", "text/event-stream")
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Last-Event-ID", progress_event_id)
        .send()
        .await?;
    assert_eq!(replay.status(), reqwest::StatusCode::OK);
    let replay_body = replay.text().await?;
    assert!(
        replay_body.contains(r#""id":2"#),
        "instance B should replay the stateless response stored by instance A: {replay_body}"
    );

    cancellation_b.cancel();
    server_b.await?;
    Ok(())
}
