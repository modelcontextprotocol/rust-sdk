//! Regression test for stateless streamable HTTP discarding the real
//! `clientInfo` from a genuine `initialize` request.
//!
//! Before the fix, `peer_info_for_stateless_request` always attached
//! `Implementation::default()` (i.e. `{"name": "rmcp", "version": "<sdk
//! version>"}`) to the synthesized `Peer`, regardless of what the client
//! actually sent in its `initialize` body. That value is what
//! `serve_inner` logs as "Service initialized as server"/"as client", and
//! what any handler sees via `context.peer.peer_info()` — so every real
//! client (regardless of its own identity) appeared identical, and
//! indistinguishable from rmcp's own placeholder, in stateless mode.
#![cfg(not(feature = "local"))]

use rmcp::{
    ErrorData, RoleServer, ServerHandler,
    model::{
        Implementation, InitializeRequestParams, InitializeResult, ServerCapabilities, ServerInfo,
    },
    service::RequestContext,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
};
use tokio_util::sync::CancellationToken;

/// Echoes both the `clientInfo` it received as a typed handler argument and
/// the `clientInfo` visible via `context.peer.peer_info()` at that same
/// instant, via `InitializeResult::instructions`, so a black-box HTTP test
/// can compare the two without any shared in-process state.
#[derive(Clone, Default)]
struct EchoingPeerInfo;

impl ServerHandler for EchoingPeerInfo {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::default())
    }

    async fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, ErrorData> {
        let peer_client_info: Option<Implementation> =
            context.peer.peer_info().map(|p| p.client_info.clone());
        let mut info = self.get_info();
        info.instructions = Some(
            serde_json::json!({
                "request_client_info": request.client_info,
                "peer_client_info": peer_client_info,
            })
            .to_string(),
        );
        Ok(info)
    }
}

fn stateless_json_config() -> StreamableHttpServerConfig {
    StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_sse_keep_alive(None)
        .with_cancellation_token(CancellationToken::new())
        .with_json_response(true)
}

async fn spawn_server_of<H: ServerHandler + Default>(
    config: StreamableHttpServerConfig,
) -> (reqwest::Client, String, CancellationToken) {
    let ct = config.cancellation_token.clone();
    let service: StreamableHttpService<H, LocalSessionManager> =
        StreamableHttpService::new(|| Ok(H::default()), Default::default(), config);

    let router = axum::Router::new().nest_service("/mcp", service);
    let tcp_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = tcp_listener.local_addr().unwrap();

    tokio::spawn({
        let ct = ct.clone();
        async move {
            let _ = axum::serve(tcp_listener, router)
                .with_graceful_shutdown(async move { ct.cancelled_owned().await })
                .await;
        }
    });

    (reqwest::Client::new(), format!("http://{addr}/mcp"), ct)
}

async fn post_init_with_client_info(
    client: &reqwest::Client,
    url: &str,
    client_name: &str,
    client_version: &str,
) -> serde_json::Value {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": client_name, "version": client_version}
        }
    });
    let resp = client
        .post(url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .body(body.to_string())
        .send()
        .await
        .expect("send request");
    assert!(resp.status().is_success(), "HTTP {}", resp.status());
    resp.json().await.expect("parse JSON")
}

#[tokio::test]
async fn stateless_init_propagates_real_client_info_to_peer_info() {
    let (client, url, ct) = spawn_server_of::<EchoingPeerInfo>(stateless_json_config()).await;

    let resp = post_init_with_client_info(&client, &url, "totally-real-client", "42.0.0").await;

    let instructions: serde_json::Value =
        serde_json::from_str(resp["result"]["instructions"].as_str().unwrap())
            .expect("instructions should contain the captured JSON");

    // Sanity check: the handler's own typed argument really did carry the
    // client's real identity — the request body was parsed correctly.
    assert_eq!(
        instructions["request_client_info"]["name"], "totally-real-client",
        "sanity check failed — the handler never saw the real clientInfo"
    );

    // The fix: `context.peer.peer_info()` — the exact value `serve_inner`
    // logs as "Service initialized as server" — must reflect the real
    // client identity for a genuine `initialize` request, not
    // `Implementation::default()`.
    assert_eq!(
        instructions["peer_client_info"]["name"], "totally-real-client",
        "Peer::peer_info() should carry the real client_info from the \
         initialize request, not a placeholder identity"
    );
    assert_eq!(instructions["peer_client_info"]["version"], "42.0.0");

    ct.cancel();
}
