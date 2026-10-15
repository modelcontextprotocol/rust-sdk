#![cfg(all(
    feature = "client",
    feature = "transport-streamable-http-client",
    feature = "transport-streamable-http-client-reqwest",
    not(feature = "local")
))]

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    Router,
    body::{Body, Bytes},
    http::{HeaderMap, Response},
    routing::post,
};
use futures::{StreamExt, stream};
use rmcp::{
    ServiceError, ServiceExt,
    model::{ClientConfig, ProtocolVersion},
    service::ClientInitializeError,
    transport::{
        StreamableHttpClientTransport,
        streamable_http_client::{StreamableHttpClientTransportConfig, StreamableHttpError},
    },
};
use serde_json::{Value, json};

fn initialize_error(id: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": -32602,
            "message": "Unsupported protocol version",
            "data": { "supported": ["2024-11-05"], "requested": "1.0.0" }
        }
    })
}

/// A single SSE event, optionally followed by a stream that never ends.
fn sse_response(message: Value, hold_open: bool) -> Response<Body> {
    let event = format!("data: {message}\n\n");
    let body = if hold_open {
        let first = stream::once(async move { Ok::<_, std::io::Error>(event) });
        Body::from_stream(first.chain(stream::pending()))
    } else {
        Body::from(event)
    };
    Response::builder()
        .status(200)
        .header("content-type", "text/event-stream")
        .body(body)
        .unwrap()
}

fn json_response(message: Value, session_id: Option<&str>) -> Response<Body> {
    let mut builder = Response::builder()
        .status(200)
        .header("content-type", "application/json");
    if let Some(session_id) = session_id {
        builder = builder.header("mcp-session-id", session_id);
    }
    builder.body(Body::from(message.to_string())).unwrap()
}

async fn spawn(router: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    format!("http://{addr}/mcp")
}

async fn spawn_sse_initialize_error_server(hold_open: bool) -> String {
    let router = Router::new().route(
        "/mcp",
        post(move |body: Bytes| async move {
            let request: Value = serde_json::from_slice(&body).unwrap();
            sse_response(initialize_error(request["id"].clone()), hold_open)
        }),
    );
    spawn(router).await
}

async fn assert_initialize_jsonrpc_error(uri: String) {
    let result = tokio::time::timeout(
        Duration::from_secs(3),
        ().serve(StreamableHttpClientTransport::from_uri(uri)),
    )
    .await
    .expect("serve() should not hang on an initialize error");
    match result {
        Err(ClientInitializeError::JsonRpcError(error)) => {
            assert_eq!(error.code.0, -32602);
            assert_eq!(error.message, "Unsupported protocol version");
            assert_eq!(
                error.data,
                Some(json!({ "supported": ["2024-11-05"], "requested": "1.0.0" }))
            );
        }
        other => panic!("expected ClientInitializeError::JsonRpcError, got: {other:?}"),
    }
}

#[tokio::test]
async fn initialize_sse_error_then_stream_end_returns_jsonrpc_error() {
    let uri = spawn_sse_initialize_error_server(false).await;
    assert_initialize_jsonrpc_error(uri).await;
}

#[tokio::test]
async fn initialize_sse_error_with_stream_held_open_returns_jsonrpc_error() {
    let uri = spawn_sse_initialize_error_server(true).await;
    assert_initialize_jsonrpc_error(uri).await;
}

type PostLog = Arc<Mutex<Vec<(String, Option<String>)>>>;

/// Accepts the first initialize as session `s1`, expires `s1` on the next
/// request, and answers the re-initialize with an error.
async fn spawn_reinit_error_server(error_over_sse: bool) -> (String, PostLog) {
    let log = PostLog::default();
    let initialized = Arc::new(Mutex::new(false));
    let router = Router::new().route(
        "/mcp",
        post({
            let log = log.clone();
            move |headers: HeaderMap, body: Bytes| {
                let log = log.clone();
                let initialized = initialized.clone();
                async move {
                    let request: Value = serde_json::from_slice(&body).unwrap();
                    let method = request["method"].as_str().unwrap_or_default().to_owned();
                    let session_id = headers
                        .get("mcp-session-id")
                        .map(|value| value.to_str().unwrap().to_owned());
                    log.lock().unwrap().push((method.clone(), session_id));
                    let id = request["id"].clone();
                    let first_initialize = method == "initialize"
                        && !std::mem::replace(&mut *initialized.lock().unwrap(), true);
                    match method.as_str() {
                        "initialize" if first_initialize => json_response(
                            json!({
                                "jsonrpc": "2.0",
                                "id": id,
                                "result": {
                                    "protocolVersion": "2025-11-25",
                                    "capabilities": {},
                                    "serverInfo": { "name": "mock", "version": "1" }
                                }
                            }),
                            Some("s1"),
                        ),
                        "initialize" if error_over_sse => sse_response(initialize_error(id), true),
                        "initialize" => json_response(initialize_error(id), None),
                        "notifications/initialized" => {
                            Response::builder().status(202).body(Body::empty()).unwrap()
                        }
                        _ => Response::builder().status(404).body(Body::empty()).unwrap(),
                    }
                }
            }
        }),
    );
    (spawn(router).await, log)
}

async fn assert_reinit_error_fails_pending_request(error_over_sse: bool) {
    let (uri, log) = spawn_reinit_error_server(error_over_sse).await;
    let transport = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(uri).reinit_on_expired_session(true),
    );
    let client = ClientConfig::default()
        .with_protocol_version(ProtocolVersion::LATEST_WITH_INITIALIZE)
        .serve(transport)
        .await
        .expect("initial handshake");

    let result = tokio::time::timeout(Duration::from_secs(3), client.list_tools(None))
        .await
        .expect("pending request should fail promptly");

    let posts = log.lock().unwrap().clone();
    let s1 = Some("s1".to_owned());
    assert_eq!(
        posts,
        vec![
            ("initialize".to_owned(), None),
            ("notifications/initialized".to_owned(), s1.clone()),
            ("tools/list".to_owned(), s1),
            ("initialize".to_owned(), None),
        ],
        "no notifications/initialized or retry may follow a failed re-initialize"
    );

    let Err(ServiceError::TransportSend(error)) = result else {
        panic!("expected TransportSend error, got: {result:?}");
    };
    match error
        .error
        .downcast_ref::<StreamableHttpError<reqwest::Error>>()
    {
        Some(StreamableHttpError::ReinitializationFailed(error)) => {
            assert_eq!(error.code.0, -32602);
            assert_eq!(error.message, "Unsupported protocol version");
            assert_eq!(
                error.data,
                Some(json!({ "supported": ["2024-11-05"], "requested": "1.0.0" }))
            );
        }
        other => panic!("expected ReinitializationFailed, got: {other:?}"),
    }
    let _ = client.cancel().await;
}

#[tokio::test]
async fn reinit_json_error_fails_pending_request_without_retry() {
    assert_reinit_error_fails_pending_request(false).await;
}

#[tokio::test]
async fn reinit_sse_error_fails_pending_request_without_retry() {
    assert_reinit_error_fails_pending_request(true).await;
}
