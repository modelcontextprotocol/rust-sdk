/// Test that demonstrates the channel replacement bug
///
/// This test reproduces the real-world scenario where VS Code reconnects SSE
/// every ~5 minutes by sending multiple GET requests with the SAME session ID.
///
/// Expected: Second GET should return 409 Conflict (like TypeScript SDK)
/// Actual: Second GET succeeds, replaces channel, orphans first receiver (BUG)
///
/// Root cause: local.rs:536 unconditionally replaces self.common.tx
use std::sync::Arc;
use std::time::Duration;

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

// Test server that sends notifications on demand
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

            println!("🔔 Server sending notification...");
            match peer.notify_tool_list_changed().await {
                Ok(()) => println!("✅ notify_tool_list_changed() returned Ok(())"),
                Err(e) => println!("❌ notify_tool_list_changed() failed: {}", e),
            }
        });
    }
}

#[tokio::test]
async fn test_channel_replacement_bug() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("debug")
        .try_init();

    let ct = CancellationToken::new();
    let notification_trigger = Arc::new(Notify::new());

    // Start HTTP server
    let server = TestServer::new(notification_trigger.clone());
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

    println!("\n=== SSE RECONNECTION BUG REPRODUCTION ===\n");
    println!("This test reproduces the real-world VS Code reconnection scenario:");
    println!("VS Code sends GET requests every ~5 minutes with the SAME session ID.");
    println!("Each GET call triggers establish_common_channel() → channel replacement → bug!\n");

    let http_client = reqwest::Client::new();

    // STEP 1: POST initialize to create session and get session ID
    println!("📡 STEP 1: POST /initialize to create session...");

    let init_request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "test-client",
                "version": "1.0.0"
            }
        }
    });

    let post_response = http_client
        .post(&url)
        .header("Accept", "text/event-stream, application/json")
        .header("Content-Type", "application/json")
        .json(&init_request)
        .timeout(Duration::from_millis(500)) // Short timeout - POST opens SSE stream
        .send()
        .await
        .expect("POST initialize");

    // In stateful mode, POST opens SSE stream and returns session ID in header
    let session_id = post_response
        .headers()
        .get("Mcp-Session-Id")
        .map(|v| v.to_str().unwrap_or("").to_string());

    if let Some(ref sid) = session_id {
        println!("✅ Session created: {}", sid);
        println!("   Response status: {}", post_response.status());
    } else {
        println!("⚠️  No Mcp-Session-Id header in POST response");
        println!("   Response status: {}", post_response.status());
        println!("   Available headers:");
        for (name, value) in post_response.headers() {
            println!("     {}: {:?}", name, value);
        }
    }

    // If no session ID from POST, we can't proceed with the test
    let session_id = session_id.expect("Session ID required for test");
    println!();

    // STEP 2: First GET with session ID to establish SSE stream.
    // IMPORTANT: We must keep this response alive so the server-side rx stays open.
    println!("📡 STEP 2: First GET (establish SSE stream)...");
    println!("   Using session: {}", session_id);

    let _get1_response = http_client
        .get(&url)
        .header("Accept", "text/event-stream")
        .header("Mcp-Session-Id", &session_id)
        .send()
        .await
        .expect("First GET request failed");

    println!("   Status: {}", _get1_response.status());
    assert!(
        _get1_response.status().is_success(),
        "First GET should succeed"
    );
    println!("   ✅ First SSE stream established (receiver listening on rx1)");

    // Give server time to set up the channel
    tokio::time::sleep(Duration::from_millis(200)).await;

    // STEP 3: Second GET with SAME session ID — should return 409 Conflict
    println!("📡 STEP 3: Second GET with SAME session ID...");
    println!("   Using session: {}", session_id);
    println!("   This simulates VS Code reconnecting after ~5 minutes");

    let get2_response = http_client
        .get(&url)
        .header("Accept", "text/event-stream")
        .header("Mcp-Session-Id", &session_id)
        .timeout(Duration::from_millis(500))
        .send()
        .await
        .expect("Second GET request failed");

    let status = get2_response.status();
    println!("   Status: {}", status);

    assert_eq!(
        status.as_u16(),
        409,
        "Second GET should return 409 Conflict (got {}). \
         Without the fix, the channel sender is silently replaced, \
         orphaning the first receiver and losing all notifications.",
        status
    );

    println!("   ✅ 409 Conflict returned (matches TypeScript SDK behavior)");

    // Cleanup
    ct.cancel();
    tokio::time::sleep(Duration::from_millis(100)).await;
}
