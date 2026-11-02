#![cfg(all(
    feature = "local",
    feature = "server",
    feature = "transport-streamable-http-server"
))]

use std::{rc::Rc, sync::Arc, time::Duration};

use bytes::Bytes;
use http::{Request, Response, StatusCode};
use http_body_util::{BodyExt, Full};
use rmcp::{
    ServerHandler,
    model::{ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig},
    service::{RequestContext, RoleServer},
    transport::{
        StreamableHttpServerConfig, StreamableHttpService,
        streamable_http_server::session::{local::LocalSessionManager, never::NeverSessionManager},
    },
};
use serde_json::{Value, json};
use tower_service::Service as _;

struct LocalServer;

impl ServerHandler for LocalServer {
    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, rmcp::ErrorData> {
        let local = Rc::new(42);
        tokio::task::yield_now().await;
        Ok(ListToolsResult {
            next_cursor: Some(local.to_string()),
            ..Default::default()
        })
    }

    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
    }
}

fn request(
    method: &str,
    version: &str,
    params: Value,
    session: Option<&str>,
) -> Request<Full<Bytes>> {
    let mut request = Request::post("http://localhost/mcp")
        .header("Host", "localhost")
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", version)
        .header("Mcp-Method", method);
    if let Some(session) = session {
        request = request.header("Mcp-Session-Id", session);
    }
    request
        .body(Full::new(Bytes::from(
            serde_json::to_vec(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": method,
                "params": params,
            }))
            .unwrap(),
        )))
        .unwrap()
}

fn list_tools_request(version: &str, session: Option<&str>) -> Request<Full<Bytes>> {
    let params = if version == "2026-07-28" {
        json!({
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": version,
                "io.modelcontextprotocol/clientCapabilities": {},
            }
        })
    } else {
        json!({})
    };
    request("tools/list", version, params, session)
}

async fn assert_tools<B>(response: Response<B>, json_response: bool)
where
    B: http_body::Body<Data = Bytes>,
    B::Error: std::fmt::Debug,
{
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["Content-Type"],
        if json_response {
            "application/json"
        } else {
            "text/event-stream"
        },
    );
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let body = std::str::from_utf8(&body).unwrap();
    let message: Value = if json_response {
        serde_json::from_str(body).unwrap()
    } else {
        body.lines()
            .filter_map(|line| line.strip_prefix("data: "))
            .find_map(|data| serde_json::from_str(data).ok())
            .expect("SSE stream contains a JSON-RPC response")
    };
    assert_eq!(message["id"], 1);
    assert_eq!(message["result"]["tools"], json!([]));
    assert_eq!(message["result"]["nextCursor"], "42");
}

async fn dispatch(json_response: bool, tower: bool) {
    tokio::task::LocalSet::new()
        .run_until(async {
            tokio::time::timeout(Duration::from_secs(5), async {
                for version in ["2026-07-28", "2025-11-25"] {
                    let config = StreamableHttpServerConfig::default()
                        .with_legacy_session_mode(false)
                        .with_json_response(json_response);
                    let mut service = StreamableHttpService::new(
                        || Ok(LocalServer),
                        Arc::new(NeverSessionManager::default()),
                        config,
                    );
                    let request = list_tools_request(version, None);
                    let response = if tower {
                        service.call(request).await.unwrap()
                    } else {
                        service.handle(request).await
                    };
                    assert_tools(response, json_response).await;
                }
            })
            .await
            .expect("local HTTP dispatch completes");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn json_handler_accepts_non_send_future() {
    dispatch(true, false).await;
}

#[tokio::test(flavor = "current_thread")]
async fn sse_handler_accepts_non_send_future() {
    dispatch(false, false).await;
}

#[tokio::test(flavor = "current_thread")]
async fn json_tower_service_accepts_non_send_future() {
    dispatch(true, true).await;
}

#[tokio::test(flavor = "current_thread")]
async fn sse_tower_service_accepts_non_send_future() {
    dispatch(false, true).await;
}

#[tokio::test(flavor = "current_thread")]
async fn legacy_session_accepts_non_send_future() {
    tokio::task::LocalSet::new()
        .run_until(async {
            tokio::time::timeout(Duration::from_secs(5), async {
                let config = StreamableHttpServerConfig::default();
                let service = StreamableHttpService::new(
                    || Ok(LocalServer),
                    Arc::new(LocalSessionManager::default()),
                    config,
                );
                let initialize = request(
                    "initialize",
                    "2025-11-25",
                    json!({
                        "protocolVersion": "2025-11-25",
                        "capabilities": {},
                        "clientInfo": {"name": "local-test", "version": "1"},
                    }),
                    None,
                );
                let response = service.handle(initialize).await;
                assert_eq!(response.status(), StatusCode::OK);
                let session = response.headers()["Mcp-Session-Id"]
                    .to_str()
                    .unwrap()
                    .to_owned();
                response.into_body().collect().await.unwrap();
                let notification = Request::post("http://localhost/mcp")
                    .header("Host", "localhost")
                    .header("Content-Type", "application/json")
                    .header("Accept", "application/json, text/event-stream")
                    .header("MCP-Protocol-Version", "2025-11-25")
                    .header("Mcp-Method", "notifications/initialized")
                    .header("Mcp-Session-Id", &session)
                    .body(Full::new(Bytes::from_static(
                        br#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
                    )))
                    .unwrap();
                assert_eq!(
                    service.handle(notification).await.status(),
                    StatusCode::ACCEPTED
                );
                let response = service
                    .handle(list_tools_request("2025-11-25", Some(&session)))
                    .await;
                assert_tools(response, false).await;
                service.config.cancellation_token.cancel();
            })
            .await
            .expect("local session dispatch completes");
        })
        .await;
}
