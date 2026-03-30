#![cfg(all(
    feature = "client",
    feature = "server",
    feature = "transport-streamable-http-client-reqwest",
    feature = "transport-streamable-http-server",
    not(feature = "local")
))]

use std::{collections::HashMap, sync::Arc};

use rmcp::{
    ServiceExt,
    transport::{
        StreamableHttpClientTransport,
        streamable_http_client::StreamableHttpClientTransportConfig,
        streamable_http_server::{
            StreamableHttpServerConfig, StreamableHttpService,
            session::{SessionState, SessionStore, SessionStoreError, local::LocalSessionManager},
        },
    },
};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

mod common;
use common::calculator::Calculator;

// ---------------------------------------------------------------------------
// Shared in-memory store used across tests
// ---------------------------------------------------------------------------

#[derive(Default, Clone)]
struct InMemorySessionStore(Arc<RwLock<HashMap<String, SessionState>>>);

impl InMemorySessionStore {
    fn new() -> Self {
        Self::default()
    }

    async fn len(&self) -> usize {
        self.0.read().await.len()
    }
}

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

// ---------------------------------------------------------------------------
// Helper: spin up a StreamableHttpService backed by the given store and
// return the bound address together with the cancellation token.
// ---------------------------------------------------------------------------

fn make_service(
    session_store: Arc<dyn SessionStore>,
    ct: &CancellationToken,
) -> StreamableHttpService<Calculator, LocalSessionManager> {
    StreamableHttpService::new(|| Ok(Calculator::new()), Default::default(), {
        let mut cfg = StreamableHttpServerConfig::default();
        cfg.stateful_mode = true;
        cfg.sse_keep_alive = None;
        cfg.cancellation_token = ct.child_token();
        cfg.session_store = Some(session_store);
        cfg
    })
}

// ---------------------------------------------------------------------------
// Test 1 — state is persisted to the store after a successful handshake
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_session_state_persisted_to_store() -> anyhow::Result<()> {
    let store = Arc::new(InMemorySessionStore::new());
    let ct = CancellationToken::new();
    let service = make_service(store.clone(), &ct);

    let router = axum::Router::new().nest_service("/mcp", service);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;

    let handle = tokio::spawn({
        let ct = ct.clone();
        async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async move { ct.cancelled_owned().await })
                .await;
        }
    });

    // Connect a full client — this performs the initialize + initialized handshake.
    let transport = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(format!("http://{addr}/mcp")),
    );
    let client = ().serve(transport).await?;

    // Make a real request so the session is fully active.
    let _resources = client.list_all_resources().await?;

    // The store should now contain exactly one session entry.
    assert_eq!(
        store.len().await,
        1,
        "session state should be persisted to the store after initialization"
    );

    // Verify the stored state contains the expected client info.
    let entries = store.0.read().await;
    let state = entries.values().next().expect("store entry should exist");
    assert_eq!(
        state.initialize_params.client_info.name, "rmcp",
        "stored client_info.name should match the rmcp client"
    );

    let _ = client.cancel().await;
    ct.cancel();
    handle.await?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Test 2 — store entry is removed when the client sends HTTP DELETE
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_session_state_deleted_from_store_on_delete() -> anyhow::Result<()> {
    let store = Arc::new(InMemorySessionStore::new());
    let session_manager = Arc::new(LocalSessionManager::default());
    let ct = CancellationToken::new();

    let service = StreamableHttpService::new(|| Ok(Calculator::new()), session_manager.clone(), {
        let mut cfg = StreamableHttpServerConfig::default();
        cfg.stateful_mode = true;
        cfg.sse_keep_alive = None;
        cfg.cancellation_token = ct.child_token();
        cfg.session_store = Some(store.clone());
        cfg
    });

    let router = axum::Router::new().nest_service("/mcp", service);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;

    let handle = tokio::spawn({
        let ct = ct.clone();
        async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async move { ct.cancelled_owned().await })
                .await;
        }
    });

    let transport = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(format!("http://{addr}/mcp")),
    );
    let client = ().serve(transport).await?;
    let _resources = client.list_all_resources().await?;

    assert_eq!(store.len().await, 1, "store should have one entry");

    // Get the session ID from the server's in-memory map.
    let session_id = {
        let sessions = session_manager.sessions.read().await;
        sessions
            .keys()
            .next()
            .cloned()
            .expect("session should exist")
    };

    // Send an explicit HTTP DELETE — this is the signal to remove from store.
    let http_client = reqwest::Client::new();
    let response = http_client
        .delete(format!("http://{addr}/mcp"))
        .header("mcp-session-id", session_id.as_ref())
        .send()
        .await?;
    assert_eq!(response.status(), 202);

    assert_eq!(
        store.len().await,
        0,
        "store entry should be removed after explicit DELETE"
    );

    let _ = client.cancel().await;
    ct.cancel();
    handle.await?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Helper: spin up a server on an ephemeral port and return its address and
// the join handle.  The server shuts down when `ct` is cancelled.
// ---------------------------------------------------------------------------

fn spawn_server(
    session_store: Option<Arc<dyn SessionStore>>,
    session_manager: Arc<LocalSessionManager>,
    ct: &CancellationToken,
) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
    let svc = StreamableHttpService::new(|| Ok(Calculator::new()), session_manager, {
        let mut cfg = StreamableHttpServerConfig::default();
        cfg.stateful_mode = true;
        cfg.sse_keep_alive = None;
        cfg.cancellation_token = ct.child_token();
        cfg.session_store = session_store;
        cfg
    });
    // Use std::net::TcpListener so the port is bound synchronously before
    // we return — avoids a race between returning the addr and the server
    // actually starting to accept connections.
    let std_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    std_listener.set_nonblocking(true).unwrap();
    let addr = std_listener.local_addr().unwrap();
    let listener = tokio::net::TcpListener::from_std(std_listener).unwrap();
    let router = axum::Router::new().nest_service("/mcp", svc);
    let handle = tokio::spawn({
        let ct = ct.clone();
        async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async move { ct.cancelled_owned().await })
                .await;
        }
    });
    (addr, handle)
}

// ---------------------------------------------------------------------------
// Test 3 — cross-instance session restore
//
// Both halves follow the same structure:
//
//   Instance A   initializes the session (session state may be saved to store)
//   Instance A   is fully shut down
//   Instance B   (fresh, no in-memory state) receives a request for the old ID
//
// Without a store → 404.  With a shared store → transparent restore.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_cross_instance_session_restore() -> anyhow::Result<()> {
    let http = reqwest::Client::new();

    // -----------------------------------------------------------------------
    // Negative check: no session store → instance B returns 404.
    // -----------------------------------------------------------------------
    {
        // --- Instance A (no store): initialize ---
        let ct_a = CancellationToken::new();
        let (addr_a, srv_a) = spawn_server(None, Arc::new(LocalSessionManager::default()), &ct_a);

        let init_resp = http
            .post(format!("http://{addr_a}/mcp"))
            .header("accept", "application/json, text/event-stream")
            .header("content-type", "application/json")
            .body(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"0"}}}"#)
            .send()
            .await?;
        assert_eq!(
            init_resp.status(),
            200,
            "instance A: initialize should succeed"
        );
        let session_id = init_resp
            .headers()
            .get("mcp-session-id")
            .expect("session ID header must be present")
            .to_str()?
            .to_owned();

        // Shut down instance A completely.
        ct_a.cancel();
        srv_a.await?;

        // --- Instance B (no store, fresh state): send request ---
        let ct_b = CancellationToken::new();
        let (addr_b, srv_b) = spawn_server(None, Arc::new(LocalSessionManager::default()), &ct_b);

        let resp = http
            .post(format!("http://{addr_b}/mcp"))
            .header("accept", "application/json, text/event-stream")
            .header("content-type", "application/json")
            .header("mcp-session-id", &session_id)
            .body(r#"{"jsonrpc":"2.0","id":2,"method":"ping","params":{}}"#)
            .send()
            .await?;
        assert_eq!(
            resp.status(),
            reqwest::StatusCode::NOT_FOUND,
            "without a session store, instance B must return 404 for an unknown session ID"
        );

        ct_b.cancel();
        srv_b.await?;
    }

    // -----------------------------------------------------------------------
    // Positive check: shared session store → instance B restores transparently.
    // -----------------------------------------------------------------------
    {
        let store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::new());

        // --- Instance A (with store): initialize ---
        let ct_a = CancellationToken::new();
        let sm_a = Arc::new(LocalSessionManager::default());
        let (addr_a, srv_a) = spawn_server(Some(store.clone()), sm_a.clone(), &ct_a);

        let init_resp = http
            .post(format!("http://{addr_a}/mcp"))
            .header("accept", "application/json, text/event-stream")
            .header("content-type", "application/json")
            .body(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"0"}}}"#)
            .send()
            .await?;
        assert_eq!(
            init_resp.status(),
            200,
            "instance A: initialize should succeed"
        );
        let original_session_id = init_resp
            .headers()
            .get("mcp-session-id")
            .expect("session ID header must be present")
            .to_str()?
            .to_owned();

        // Confirm the session was persisted.
        let store_ref = store
            .load(&original_session_id)
            .await
            .expect("store load should not error");
        assert!(
            store_ref.is_some(),
            "store should hold the session after initialization"
        );

        // Shut down instance A completely — session lives only in the store now.
        ct_a.cancel();
        srv_a.await?;

        // --- Instance B (same store, fresh in-memory state): send request ---
        let ct_b = CancellationToken::new();
        let sm_b = Arc::new(LocalSessionManager::default());
        let (addr_b, srv_b) = spawn_server(Some(store.clone()), sm_b.clone(), &ct_b);

        let resp = http
            .post(format!("http://{addr_b}/mcp"))
            .header("accept", "application/json, text/event-stream")
            .header("content-type", "application/json")
            .header("mcp-session-id", &original_session_id)
            .body(r#"{"jsonrpc":"2.0","id":2,"method":"ping","params":{}}"#)
            .send()
            .await?;
        assert_eq!(
            resp.status(),
            200,
            "instance B: request must succeed after transparent restore"
        );

        // The session must be in instance B's memory under the ORIGINAL ID.
        {
            let sessions = sm_b.sessions.read().await;
            let restored_id = sessions
                .keys()
                .next()
                .expect("session should exist in instance B after restore");
            assert_eq!(
                restored_id.as_ref(),
                original_session_id.as_str(),
                "restored session must keep the original session ID"
            );
        }

        ct_b.cancel();
        srv_b.await?;
    }

    Ok(())
}
