/// Tests for concurrent SSE stream handling (shadow channels)
///
/// These tests verify that multiple GET SSE streams on the same session
/// don't kill each other by replacing the common channel sender.
///
/// Root cause: When POST SSE responses include `retry`, the EventSource API
/// reconnects via GET after the stream ends. Each GET was unconditionally
/// replacing `self.common.tx`, killing the other stream's receiver — causing
/// an infinite reconnect loop every `sse_retry` seconds.
///
/// Fix: `resume_or_shadow_common()` checks if the primary common channel is
/// still active. If so, it creates a "shadow" stream (idle, keep-alive only)
/// instead of replacing the primary.
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use reqwest;
use rmcp::{
    RoleServer, ServerHandler,
    model::{Implementation, ProtocolVersion, ServerCapabilities, ServerInfo, ToolsCapability},
    service::NotificationContext,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
};
use serde_json::json;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

const ACCEPT_SSE: &str = "text/event-stream";
const ACCEPT_BOTH: &str = "text/event-stream, application/json";

// ─── Test server ────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct TestServer {
    trigger: Arc<Notify>,
}

impl TestServer {
    fn new(trigger: Arc<Notify>) -> Self {
        Self { trigger }
    }
}

impl ServerHandler for TestServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::LATEST,
            capabilities: ServerCapabilities::builder()
                .enable_tools_with(ToolsCapability {
                    list_changed: Some(true),
                })
                .build(),
            server_info: Implementation {
                name: "test-server".to_string(),
                version: "1.0.0".to_string(),
                ..Default::default()
            },
            instructions: None,
        }
    }

    async fn on_initialized(&self, context: NotificationContext<RoleServer>) {
        let peer = context.peer.clone();
        let trigger = self.trigger.clone();

        tokio::spawn(async move {
            trigger.notified().await;
            let _ = peer.notify_tool_list_changed().await;
        });
    }
}

// ─── Helpers ────────────────────────────────────────────────────────────────

async fn start_test_server(ct: CancellationToken, trigger: Arc<Notify>) -> String {
    let server = TestServer::new(trigger);
    let service = StreamableHttpService::new(
        move || Ok(server.clone()),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig {
            stateful_mode: true,
            sse_keep_alive: Some(Duration::from_secs(15)),
            sse_retry: Some(Duration::from_secs(3)),
            cancellation_token: ct.child_token(),
        },
    );

    let router = axum::Router::new().nest_service("/mcp", service);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().unwrap();
    let url = format!("http://127.0.0.1:{}/mcp", addr.port());

    let ct_clone = ct.clone();
    tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async move { ct_clone.cancelled().await })
            .await
            .unwrap();
    });

    tokio::time::sleep(Duration::from_millis(100)).await;
    url
}

/// POST initialize and return session ID.
async fn initialize_session(client: &reqwest::Client, url: &str) -> String {
    let resp = client
        .post(url)
        .header("Accept", ACCEPT_BOTH)
        .header("Content-Type", "application/json")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "test-client", "version": "1.0.0" }
            }
        }))
        .timeout(Duration::from_millis(500))
        .send()
        .await
        .expect("POST initialize");

    assert!(resp.status().is_success(), "initialize should succeed");

    resp.headers()
        .get("Mcp-Session-Id")
        .expect("session ID header")
        .to_str()
        .unwrap()
        .to_string()
}

/// POST `notifications/initialized` to complete the MCP handshake.
/// This triggers the server's `on_initialized` handler.
async fn send_initialized_notification(client: &reqwest::Client, url: &str, session_id: &str) {
    let resp = client
        .post(url)
        .header("Accept", ACCEPT_BOTH)
        .header("Content-Type", "application/json")
        .header("Mcp-Session-Id", session_id)
        .json(&json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }))
        .send()
        .await
        .expect("POST notifications/initialized");

    assert_eq!(
        resp.status().as_u16(),
        202,
        "notifications/initialized should return 202 Accepted"
    );
}

/// Open a standalone GET SSE stream (no Last-Event-ID).
async fn open_standalone_get(
    client: &reqwest::Client,
    url: &str,
    session_id: &str,
) -> reqwest::Response {
    client
        .get(url)
        .header("Accept", ACCEPT_SSE)
        .header("Mcp-Session-Id", session_id)
        .send()
        .await
        .expect("GET SSE stream")
}

/// Open a GET SSE stream with Last-Event-ID (resume).
async fn open_resume_get(
    client: &reqwest::Client,
    url: &str,
    session_id: &str,
    last_event_id: &str,
) -> reqwest::Response {
    client
        .get(url)
        .header("Accept", ACCEPT_SSE)
        .header("Mcp-Session-Id", session_id)
        .header("Last-Event-ID", last_event_id)
        .send()
        .await
        .expect("GET SSE stream with Last-Event-ID")
}

/// Read from an SSE byte stream until we find a specific text or timeout.
async fn wait_for_sse_event(resp: reqwest::Response, needle: &str, timeout: Duration) -> bool {
    let mut stream = resp.bytes_stream();
    let result = tokio::time::timeout(timeout, async {
        while let Some(Ok(chunk)) = stream.next().await {
            let text = String::from_utf8_lossy(&chunk);
            if text.contains(needle) {
                return true;
            }
        }
        false
    })
    .await;

    matches!(result, Ok(true))
}

// ─── Tests: Shadow stream creation ──────────────────────────────────────────

/// Second standalone GET with same session ID should return 200 OK
/// (shadow stream), NOT 409 Conflict.
#[tokio::test]
async fn shadow_second_standalone_get_returns_200() {
    let ct = CancellationToken::new();
    let trigger = Arc::new(Notify::new());
    let url = start_test_server(ct.clone(), trigger).await;
    let client = reqwest::Client::new();

    let session_id = initialize_session(&client, &url).await;

    // First GET — becomes primary common channel
    let get1 = open_standalone_get(&client, &url, &session_id).await;
    assert_eq!(get1.status(), 200, "First GET should succeed");

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Second GET — should get 200 (shadow), NOT 409
    let get2 = open_standalone_get(&client, &url, &session_id).await;
    assert_eq!(
        get2.status(),
        200,
        "Second GET should return 200 (shadow stream), not 409 Conflict"
    );

    ct.cancel();
}

/// Multiple standalone GETs should all return 200 — the server can handle
/// many shadow streams concurrently.
#[tokio::test]
async fn shadow_multiple_standalone_gets_all_succeed() {
    let ct = CancellationToken::new();
    let trigger = Arc::new(Notify::new());
    let url = start_test_server(ct.clone(), trigger).await;
    let client = reqwest::Client::new();

    let session_id = initialize_session(&client, &url).await;

    // Open 5 concurrent standalone GETs
    let mut responses = Vec::new();
    for i in 0..5 {
        let resp = open_standalone_get(&client, &url, &session_id).await;
        assert_eq!(resp.status(), 200, "GET #{i} should succeed");
        responses.push(resp);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // All 5 should be alive (first is primary, rest are shadows)
    assert_eq!(responses.len(), 5);

    ct.cancel();
}

// ─── Tests: Dead primary replacement ────────────────────────────────────────

/// When the primary common channel is dead (first GET dropped), the next GET
/// should replace it and become the new primary.
#[tokio::test]
async fn dead_primary_gets_replaced_by_next_get() {
    let ct = CancellationToken::new();
    let trigger = Arc::new(Notify::new());
    let url = start_test_server(ct.clone(), trigger).await;
    let client = reqwest::Client::new();

    let session_id = initialize_session(&client, &url).await;

    // First GET — becomes primary
    let get1 = open_standalone_get(&client, &url, &session_id).await;
    assert_eq!(get1.status(), 200);

    // Drop primary — kills receiver, making sender closed
    drop(get1);
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Second GET — primary is dead, should replace it
    let get2 = open_standalone_get(&client, &url, &session_id).await;
    assert_eq!(
        get2.status(),
        200,
        "GET should succeed as new primary after old primary was dropped"
    );

    ct.cancel();
}

/// After primary dies, the replacement primary should be able to receive
/// notifications (verifies the channel was actually replaced, not shadowed).
#[tokio::test]
async fn dead_primary_replacement_receives_notifications() {
    let ct = CancellationToken::new();
    let trigger = Arc::new(Notify::new());
    let url = start_test_server(ct.clone(), trigger.clone()).await;
    let client = reqwest::Client::new();

    let session_id = initialize_session(&client, &url).await;
    send_initialized_notification(&client, &url, &session_id).await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    // First GET — becomes primary
    let get1 = open_standalone_get(&client, &url, &session_id).await;
    assert_eq!(get1.status(), 200);

    // Drop primary
    drop(get1);
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Second GET — becomes new primary (replacement)
    let get2 = open_standalone_get(&client, &url, &session_id).await;
    assert_eq!(get2.status(), 200);

    // Trigger notification — should arrive on get2 (the new primary)
    trigger.notify_one();

    assert!(
        wait_for_sse_event(get2, "tools/list_changed", Duration::from_secs(3)).await,
        "Replacement primary should receive notifications"
    );

    ct.cancel();
}

/// Multiple drops and replacements should work: primary can be replaced
/// more than once.
#[tokio::test]
async fn dead_primary_can_be_replaced_multiple_times() {
    let ct = CancellationToken::new();
    let trigger = Arc::new(Notify::new());
    let url = start_test_server(ct.clone(), trigger).await;
    let client = reqwest::Client::new();

    let session_id = initialize_session(&client, &url).await;

    for i in 0..3 {
        let get = open_standalone_get(&client, &url, &session_id).await;
        assert_eq!(get.status(), 200, "GET #{i} should succeed");
        drop(get);
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Final GET should still work
    let final_get = open_standalone_get(&client, &url, &session_id).await;
    assert_eq!(
        final_get.status(),
        200,
        "GET after multiple replacements should succeed"
    );

    ct.cancel();
}

// ─── Tests: Notification routing ────────────────────────────────────────────

/// Notification should arrive on the primary stream even after shadow streams
/// are created by subsequent GETs.
#[tokio::test]
async fn notification_reaches_primary_not_shadow() {
    let ct = CancellationToken::new();
    let trigger = Arc::new(Notify::new());
    let url = start_test_server(ct.clone(), trigger.clone()).await;
    let client = reqwest::Client::new();

    let session_id = initialize_session(&client, &url).await;
    send_initialized_notification(&client, &url, &session_id).await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    // First GET — primary common channel
    let get1 = open_standalone_get(&client, &url, &session_id).await;
    assert_eq!(get1.status(), 200);
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Second GET — shadow stream (should NOT steal notifications)
    let _get2 = open_standalone_get(&client, &url, &session_id).await;
    assert_eq!(_get2.status(), 200);
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Trigger notification
    trigger.notify_one();

    // Primary stream should receive the notification
    assert!(
        wait_for_sse_event(get1, "tools/list_changed", Duration::from_secs(3)).await,
        "Primary stream should receive notification even after shadow was created"
    );

    ct.cancel();
}

// ─── Tests: Resume with Last-Event-ID ───────────────────────────────────────

/// GET with Last-Event-ID referencing a completed request-wise channel should
/// fall through to shadow (not crash or return 500).
///
/// This simulates the real-world scenario: POST SSE response ends, the
/// EventSource reconnects via GET with the last event ID from the POST stream.
/// The request-wise channel no longer exists, so the server should create a
/// shadow stream.
#[tokio::test]
async fn resume_completed_request_wise_creates_shadow() {
    let ct = CancellationToken::new();
    let trigger = Arc::new(Notify::new());
    let url = start_test_server(ct.clone(), trigger).await;
    let client = reqwest::Client::new();

    let session_id = initialize_session(&client, &url).await;

    // First GET — establish primary
    let _get1 = open_standalone_get(&client, &url, &session_id).await;
    assert_eq!(_get1.status(), 200);
    tokio::time::sleep(Duration::from_millis(100)).await;

    // GET with Last-Event-ID for non-existent request-wise channel
    let get_resume = open_resume_get(&client, &url, &session_id, "0/999").await;
    assert_eq!(
        get_resume.status(),
        200,
        "Resume of completed request-wise channel should return 200 (shadow)"
    );

    ct.cancel();
}

/// GET with Last-Event-ID "0" (common channel resume) while primary is alive
/// should create a shadow.
#[tokio::test]
async fn resume_common_while_primary_alive_creates_shadow() {
    let ct = CancellationToken::new();
    let trigger = Arc::new(Notify::new());
    let url = start_test_server(ct.clone(), trigger).await;
    let client = reqwest::Client::new();

    let session_id = initialize_session(&client, &url).await;

    // First GET — establish primary
    let _get1 = open_standalone_get(&client, &url, &session_id).await;
    assert_eq!(_get1.status(), 200);
    tokio::time::sleep(Duration::from_millis(100)).await;

    // GET with Last-Event-ID "0" — resume common while primary alive → shadow
    let get_resume = open_resume_get(&client, &url, &session_id, "0").await;
    assert_eq!(
        get_resume.status(),
        200,
        "Common channel resume while primary alive should return 200 (shadow)"
    );

    ct.cancel();
}

/// GET with Last-Event-ID "0" (common channel resume) while primary is dead
/// should become the new primary.
#[tokio::test]
async fn resume_common_while_primary_dead_becomes_primary() {
    let ct = CancellationToken::new();
    let trigger = Arc::new(Notify::new());
    let url = start_test_server(ct.clone(), trigger.clone()).await;
    let client = reqwest::Client::new();

    let session_id = initialize_session(&client, &url).await;
    send_initialized_notification(&client, &url, &session_id).await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    // First GET — establish primary
    let get1 = open_standalone_get(&client, &url, &session_id).await;
    assert_eq!(get1.status(), 200);

    // Drop primary
    drop(get1);
    tokio::time::sleep(Duration::from_millis(100)).await;

    // GET with Last-Event-ID "0" — primary dead → becomes new primary
    let get_resume = open_resume_get(&client, &url, &session_id, "0").await;
    assert_eq!(get_resume.status(), 200);

    // New primary should receive notifications
    trigger.notify_one();

    assert!(
        wait_for_sse_event(get_resume, "tools/list_changed", Duration::from_secs(3)).await,
        "Resumed stream that replaced dead primary should receive notifications"
    );

    ct.cancel();
}

// ─── Tests: Mixed scenarios ─────────────────────────────────────────────────

/// POST SSE reconnections and standalone GET should coexist: POST initialize
/// creates a request-wise channel, its EventSource reconnects via GET after
/// the stream ends, while a standalone GET is also active.
#[tokio::test]
async fn post_reconnect_and_standalone_coexist() {
    let ct = CancellationToken::new();
    let trigger = Arc::new(Notify::new());
    let url = start_test_server(ct.clone(), trigger).await;
    let client = reqwest::Client::new();

    let session_id = initialize_session(&client, &url).await;

    // Standalone GET — becomes primary
    let _standalone = open_standalone_get(&client, &url, &session_id).await;
    assert_eq!(_standalone.status(), 200);
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Simulate POST SSE response reconnection (EventSource reconnects with
    // Last-Event-ID from the initialize POST stream). The request-wise channel
    // for the initialize request is already completed.
    let reconnect1 = open_resume_get(&client, &url, &session_id, "0/0").await;
    assert_eq!(
        reconnect1.status(),
        200,
        "POST reconnection should get shadow, not replace primary"
    );

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Another POST reconnection (e.g. from tools/list response)
    let reconnect2 = open_resume_get(&client, &url, &session_id, "0/1").await;
    assert_eq!(
        reconnect2.status(),
        200,
        "Second POST reconnection should also succeed"
    );

    ct.cancel();
}

/// Standalone GET is dropped (e.g. client timeout), a new standalone GET
/// connects. The new one should become the primary and receive notifications.
#[tokio::test]
async fn reconnect_after_stream_timeout() {
    let ct = CancellationToken::new();
    let trigger = Arc::new(Notify::new());
    let url = start_test_server(ct.clone(), trigger.clone()).await;
    let client = reqwest::Client::new();

    let session_id = initialize_session(&client, &url).await;
    send_initialized_notification(&client, &url, &session_id).await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    // First standalone GET — primary
    let get1 = open_standalone_get(&client, &url, &session_id).await;
    assert_eq!(get1.status(), 200);

    // Client drops the stream (e.g. timeout or reconnection)
    drop(get1);
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Client reconnects with a new standalone GET
    let get2 = open_standalone_get(&client, &url, &session_id).await;
    assert_eq!(get2.status(), 200);

    // Notification should reach the new primary
    trigger.notify_one();

    assert!(
        wait_for_sse_event(get2, "tools/list_changed", Duration::from_secs(3)).await,
        "Reconnected stream should receive notifications"
    );

    ct.cancel();
}

// ─── Tests: Edge cases ──────────────────────────────────────────────────────

/// GET with an unknown session ID should return 404 Not Found per MCP spec.
/// This signals the client to re-initialize (not re-authenticate).
#[tokio::test]
async fn get_without_valid_session_returns_404() {
    let ct = CancellationToken::new();
    let trigger = Arc::new(Notify::new());
    let url = start_test_server(ct.clone(), trigger).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(&url)
        .header("Accept", ACCEPT_SSE)
        .header("Mcp-Session-Id", "nonexistent-session-id")
        .send()
        .await
        .expect("GET with invalid session");

    assert_eq!(
        resp.status().as_u16(),
        404,
        "GET with unknown session ID should return 404 Not Found per MCP spec"
    );

    ct.cancel();
}

/// GET without session ID header should return 400 Bad Request per MCP spec.
#[tokio::test]
async fn get_without_session_id_header_returns_400() {
    let ct = CancellationToken::new();
    let trigger = Arc::new(Notify::new());
    let url = start_test_server(ct.clone(), trigger).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(&url)
        .header("Accept", ACCEPT_SSE)
        .send()
        .await
        .expect("GET without session ID");

    assert_eq!(
        resp.status().as_u16(),
        400,
        "GET without session ID should return 400 Bad Request per MCP spec"
    );

    ct.cancel();
}

/// Shadow streams should be idle — they should NOT receive notifications.
/// Only the primary receives them.
#[tokio::test]
async fn shadow_stream_does_not_receive_notifications() {
    let ct = CancellationToken::new();
    let trigger = Arc::new(Notify::new());
    let url = start_test_server(ct.clone(), trigger.clone()).await;
    let client = reqwest::Client::new();

    let session_id = initialize_session(&client, &url).await;
    send_initialized_notification(&client, &url, &session_id).await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    // First GET — primary
    let _get1 = open_standalone_get(&client, &url, &session_id).await;
    assert_eq!(_get1.status(), 200);
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Second GET — shadow
    let get2 = open_standalone_get(&client, &url, &session_id).await;
    assert_eq!(get2.status(), 200);
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Trigger notification
    trigger.notify_one();

    // Shadow stream should NOT receive the notification (timeout expected)
    let shadow_received =
        wait_for_sse_event(get2, "tools/list_changed", Duration::from_millis(500)).await;
    assert!(
        !shadow_received,
        "Shadow stream should NOT receive notifications"
    );

    ct.cancel();
}

/// Dropping all shadow streams should not affect the primary channel.
/// Primary should still receive notifications after all shadows are dropped.
#[tokio::test]
async fn dropping_shadows_does_not_affect_primary() {
    let ct = CancellationToken::new();
    let trigger = Arc::new(Notify::new());
    let url = start_test_server(ct.clone(), trigger.clone()).await;
    let client = reqwest::Client::new();

    let session_id = initialize_session(&client, &url).await;
    send_initialized_notification(&client, &url, &session_id).await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Primary GET
    let get1 = open_standalone_get(&client, &url, &session_id).await;
    assert_eq!(get1.status(), 200);
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Create and drop several shadows
    for _ in 0..3 {
        let shadow = open_standalone_get(&client, &url, &session_id).await;
        assert_eq!(shadow.status(), 200);
        drop(shadow);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // Trigger notification — primary should still receive it
    trigger.notify_one();

    assert!(
        wait_for_sse_event(get1, "tools/list_changed", Duration::from_secs(3)).await,
        "Primary should still work after all shadows are dropped"
    );

    ct.cancel();
}

// ─── Tests: Cache replay on dead primary replacement ─────────────────────────

/// When a notification is sent while the primary is alive, then the primary
/// dies and a new GET resumes with Last-Event-ID "0", the replacement primary
/// should receive the cached notification via sync() replay.
#[tokio::test]
async fn dead_primary_replacement_replays_cached_events() {
    let ct = CancellationToken::new();
    let trigger = Arc::new(Notify::new());
    let url = start_test_server(ct.clone(), trigger.clone()).await;
    let client = reqwest::Client::new();

    let session_id = initialize_session(&client, &url).await;
    send_initialized_notification(&client, &url, &session_id).await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    // First GET — becomes primary
    let get1 = open_standalone_get(&client, &url, &session_id).await;
    assert_eq!(get1.status(), 200);
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Trigger notification while primary is alive (gets cached)
    trigger.notify_one();
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Drop primary — notification was sent and cached
    drop(get1);
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Resume with Last-Event-ID "0" — primary is dead, should replace it
    // and replay cached events from index 0
    let get_resume = open_resume_get(&client, &url, &session_id, "0").await;
    assert_eq!(get_resume.status(), 200);

    // The cached notification should be replayed on the new primary
    assert!(
        wait_for_sse_event(get_resume, "tools/list_changed", Duration::from_secs(3)).await,
        "Replacement primary should receive cached notification via sync() replay"
    );

    ct.cancel();
}
