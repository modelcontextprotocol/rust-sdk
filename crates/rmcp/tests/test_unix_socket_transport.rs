#![cfg(all(
    unix,
    feature = "transport-streamable-http-client-unix-socket",
    not(feature = "local")
))]

use std::{collections::HashMap, sync::Arc};

use axum::{
    Router, body::Bytes, extract::State, http::StatusCode, response::IntoResponse, routing::post,
};
use http::{HeaderName, HeaderValue};
use hyper_util::rt::TokioIo;
use rmcp::{
    ServiceExt,
    model::{
        ClientJsonRpcMessage, ClientNotification, ClientRequest, InitializedNotification,
        PingRequest, RequestId,
    },
    transport::{
        StreamableHttpClientTransport, UnixSocketHttpClient,
        streamable_http_client::{
            StreamableHttpClient, StreamableHttpClientTransportConfig, StreamableHttpError,
            StreamableHttpPostResponse,
        },
    },
};
use serde_json::json;
use tokio::sync::Mutex;

#[derive(Clone)]
struct ServerState {
    received_headers: Arc<Mutex<HashMap<String, String>>>,
    initialize_called: Arc<tokio::sync::Notify>,
}

async fn mcp_handler(
    State(state): State<ServerState>,
    headers: http::HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let mut headers_map = HashMap::new();
    for (name, value) in headers.iter() {
        let name_str = name.as_str();
        if (name_str.starts_with("x-") || name_str == "host")
            && let Ok(v) = value.to_str()
        {
            headers_map.insert(name_str.to_string(), v.to_string());
        }
    }

    let mut stored = state.received_headers.lock().await;
    stored.extend(headers_map);
    drop(stored);

    if let Ok(json_body) = serde_json::from_slice::<serde_json::Value>(&body)
        && let Some(method) = json_body.get("method").and_then(|m| m.as_str())
    {
        if method == "initialize" {
            state.initialize_called.notify_one();
            let response = json!({
                "jsonrpc": "2.0",
                "id": json_body.get("id"),
                "result": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "serverInfo": {
                        "name": "test-unix-server",
                        "version": "1.0.0"
                    }
                }
            });
            return (
                StatusCode::OK,
                [
                    (http::header::CONTENT_TYPE, "application/json"),
                    (
                        http::HeaderName::from_static("mcp-session-id"),
                        "unix-test-session",
                    ),
                ],
                response.to_string(),
            );
        } else if method == "notifications/initialized" {
            return (
                StatusCode::ACCEPTED,
                [
                    (http::header::CONTENT_TYPE, "application/json"),
                    (
                        http::HeaderName::from_static("mcp-session-id"),
                        "unix-test-session",
                    ),
                ],
                String::new(),
            );
        }
    }

    let request_id = serde_json::from_slice::<serde_json::Value>(&body)
        .ok()
        .and_then(|j| j.get("id").cloned())
        .unwrap_or(serde_json::Value::Null);
    let response = json!({
        "jsonrpc": "2.0",
        "id": request_id,
        "result": {}
    });
    (
        StatusCode::OK,
        [
            (http::header::CONTENT_TYPE, "application/json"),
            (
                http::HeaderName::from_static("mcp-session-id"),
                "unix-test-session",
            ),
        ],
        response.to_string(),
    )
}

/// Spawns an HTTP/1.1 server on a Unix socket using hyper directly.
/// Avoids `axum::serve(UnixListener, ...)` which uses `spawn_local` on Linux.
fn spawn_unix_server(
    listener: tokio::net::UnixListener,
    app: Router,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let tower_service = app.clone();
            tokio::spawn(async move {
                let io = TokioIo::new(stream);
                let hyper_service = hyper::service::service_fn(
                    move |req: hyper::Request<hyper::body::Incoming>| {
                        let mut tower_service = tower_service.clone();
                        async move {
                            use tower_service::Service;
                            tower_service.call(req).await
                        }
                    },
                );
                hyper::server::conn::http1::Builder::new()
                    .serve_connection(io, hyper_service)
                    .await
                    .ok();
            });
        }
    })
}

/// Integration test: MCP client connects and completes handshake over a Unix domain socket.
#[tokio::test]
async fn test_unix_socket_mcp_handshake() -> anyhow::Result<()> {
    let dir = std::env::temp_dir().join(format!("rmcp-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let socket_path = dir.join("mcp.sock");

    let _ = std::fs::remove_file(&socket_path);

    let state = ServerState {
        received_headers: Arc::new(Mutex::new(HashMap::new())),
        initialize_called: Arc::new(tokio::sync::Notify::new()),
    };

    let app = Router::new()
        .route("/mcp", post(mcp_handler))
        .with_state(state.clone());

    let listener = tokio::net::UnixListener::bind(&socket_path)?;
    let server_handle = spawn_unix_server(listener, app);

    let socket_str = socket_path.to_str().unwrap();
    let uri = "http://mcp-server.internal/mcp";
    let client = UnixSocketHttpClient::new(socket_str, uri);
    let config = StreamableHttpClientTransportConfig::with_uri(uri);
    let transport = StreamableHttpClientTransport::with_client(client, config);

    let mcp_client = ().serve(transport).await.expect("MCP handshake should succeed");

    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        state.initialize_called.notified(),
    )
    .await
    .expect("Initialize request should be received");

    let headers = state.received_headers.lock().await;
    assert_eq!(
        headers.get("host"),
        Some(&"mcp-server.internal".to_string()),
        "Host header should be derived from URI"
    );

    drop(mcp_client);
    server_handle.abort();
    let _ = std::fs::remove_file(&socket_path);
    let _ = std::fs::remove_dir(&dir);

    Ok(())
}

/// Integration test: Custom headers are sent through the Unix socket transport.
#[tokio::test]
async fn test_unix_socket_custom_headers() -> anyhow::Result<()> {
    let dir = std::env::temp_dir().join(format!("rmcp-test-headers-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let socket_path = dir.join("mcp.sock");
    let _ = std::fs::remove_file(&socket_path);

    let state = ServerState {
        received_headers: Arc::new(Mutex::new(HashMap::new())),
        initialize_called: Arc::new(tokio::sync::Notify::new()),
    };

    let app = Router::new()
        .route("/mcp", post(mcp_handler))
        .with_state(state.clone());

    let listener = tokio::net::UnixListener::bind(&socket_path)?;
    let server_handle = spawn_unix_server(listener, app);

    let mut custom_headers = HashMap::new();
    custom_headers.insert(
        HeaderName::from_static("x-test-header"),
        HeaderValue::from_static("test-value-123"),
    );
    custom_headers.insert(
        HeaderName::from_static("x-client-id"),
        HeaderValue::from_static("unix-test-client"),
    );

    let socket_str = socket_path.to_str().unwrap();
    let uri = "http://mcp-server.internal/mcp";
    let client = UnixSocketHttpClient::new(socket_str, uri);
    let config = StreamableHttpClientTransportConfig::with_uri(uri).custom_headers(custom_headers);
    let transport = StreamableHttpClientTransport::with_client(client, config);

    let mcp_client = ().serve(transport).await.expect("MCP handshake should succeed");

    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        state.initialize_called.notified(),
    )
    .await
    .expect("Initialize request should be received");

    let headers = state.received_headers.lock().await;
    assert_eq!(
        headers.get("x-test-header"),
        Some(&"test-value-123".to_string()),
        "Custom header x-test-header should be received"
    );
    assert_eq!(
        headers.get("x-client-id"),
        Some(&"unix-test-client".to_string()),
        "Custom header x-client-id should be received"
    );

    drop(mcp_client);
    server_handle.abort();
    let _ = std::fs::remove_file(&socket_path);
    let _ = std::fs::remove_dir(&dir);

    Ok(())
}

/// Integration test: Convenience constructor `from_unix_socket` works end-to-end.
#[tokio::test]
async fn test_unix_socket_convenience_constructor() -> anyhow::Result<()> {
    let dir = std::env::temp_dir().join(format!("rmcp-test-conv-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let socket_path = dir.join("mcp.sock");
    let _ = std::fs::remove_file(&socket_path);

    let state = ServerState {
        received_headers: Arc::new(Mutex::new(HashMap::new())),
        initialize_called: Arc::new(tokio::sync::Notify::new()),
    };

    let app = Router::new()
        .route("/mcp", post(mcp_handler))
        .with_state(state.clone());

    let listener = tokio::net::UnixListener::bind(&socket_path)?;
    let server_handle = spawn_unix_server(listener, app);

    let socket_str = socket_path.to_str().unwrap();
    let transport =
        StreamableHttpClientTransport::from_unix_socket(socket_str, "http://localhost/mcp");

    let mcp_client = ().serve(transport).await.expect("MCP handshake should succeed");

    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        state.initialize_called.notified(),
    )
    .await
    .expect("Initialize request should be received");

    drop(mcp_client);
    server_handle.abort();
    let _ = std::fs::remove_file(&socket_path);
    let _ = std::fs::remove_dir(&dir);

    Ok(())
}

/// A request POST that gets 200 + `application/json` + `{}` must error.
/// Mapping that body to Accepted would hang the caller waiting on SSE.
#[tokio::test]
async fn test_unix_socket_request_malformed_json_is_unexpected() -> anyhow::Result<()> {
    let dir = std::env::temp_dir().join(format!("rmcp-test-bad-json-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let socket_path = dir.join("mcp.sock");
    let _ = std::fs::remove_file(&socket_path);

    async fn bad_json_handler() -> impl IntoResponse {
        (
            StatusCode::OK,
            [(http::header::CONTENT_TYPE, "application/json")],
            "{}",
        )
    }

    let app = Router::new().route("/mcp", post(bad_json_handler));
    let listener = tokio::net::UnixListener::bind(&socket_path)?;
    let server_handle = spawn_unix_server(listener, app);

    let socket_str = socket_path.to_str().unwrap();
    let uri = "http://localhost/mcp";
    let client = UnixSocketHttpClient::new(socket_str, uri);
    let result = client
        .post_message(
            Arc::from(uri),
            ClientJsonRpcMessage::request(
                ClientRequest::PingRequest(PingRequest::default()),
                RequestId::Number(1),
            ),
            None,
            None,
            HashMap::new(),
        )
        .await;

    server_handle.abort();
    let _ = std::fs::remove_file(&socket_path);
    let _ = std::fs::remove_dir(&dir);

    match result {
        Err(StreamableHttpError::UnexpectedServerResponse(ref msg)) => {
            assert!(
                msg.contains("{}"),
                "expected body preview in error, got: {msg}"
            );
        }
        other => panic!("expected UnexpectedServerResponse, got: {other:?}"),
    }
    Ok(())
}

/// A notification POST answered 200 + `text/plain` must be `Accepted`: a
/// notification awaits no reply, so the response body is not load-bearing.
/// This exercises the unix-socket transport's copy of the fallback, which the
/// 202-answering handler in `mcp_handler` never reaches. The whole file is
/// `cfg(unix)`, so this case runs on Linux CI and is compiled out on Windows.
#[tokio::test]
async fn test_unix_socket_notification_text_plain_success_is_accepted() -> anyhow::Result<()> {
    let dir = std::env::temp_dir().join(format!("rmcp-test-unix-text-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let socket_path = dir.join("mcp.sock");
    let _ = std::fs::remove_file(&socket_path);

    async fn text_plain_handler() -> impl IntoResponse {
        (StatusCode::OK, "this is not a JSON-RPC message")
    }

    let app = Router::new().route("/mcp", post(text_plain_handler));
    let listener = tokio::net::UnixListener::bind(&socket_path)?;
    let server_handle = spawn_unix_server(listener, app);

    let socket_str = socket_path.to_str().unwrap();
    let uri = "http://localhost/mcp";
    let client = UnixSocketHttpClient::new(socket_str, uri);
    let result = client
        .post_message(
            Arc::from(uri),
            ClientJsonRpcMessage::notification(ClientNotification::InitializedNotification(
                InitializedNotification::default(),
            )),
            None,
            None,
            HashMap::new(),
        )
        .await;

    server_handle.abort();
    let _ = std::fs::remove_file(&socket_path);
    let _ = std::fs::remove_dir(&dir);

    match result {
        Ok(StreamableHttpPostResponse::Accepted) => {}
        other => panic!("expected Accepted, got: {other:?}"),
    }
    Ok(())
}

/// A notification POST answered 200 with neither `Content-Type` nor
/// `Content-Length` (chunked empty body) must also be `Accepted`; this is the
/// shape the `Content-Length == 0` shortcut cannot detect. Runs on Linux CI.
#[tokio::test]
async fn test_unix_socket_notification_without_content_type_is_accepted() -> anyhow::Result<()> {
    let dir = std::env::temp_dir().join(format!("rmcp-test-unix-chunked-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let socket_path = dir.join("mcp.sock");
    let _ = std::fs::remove_file(&socket_path);

    async fn chunked_empty_handler() -> impl IntoResponse {
        axum::body::Body::from_stream(futures::stream::empty::<Result<Bytes, std::io::Error>>())
    }

    let app = Router::new().route("/mcp", post(chunked_empty_handler));
    let listener = tokio::net::UnixListener::bind(&socket_path)?;
    let server_handle = spawn_unix_server(listener, app);

    let socket_str = socket_path.to_str().unwrap();
    let uri = "http://localhost/mcp";
    let client = UnixSocketHttpClient::new(socket_str, uri);
    let result = client
        .post_message(
            Arc::from(uri),
            ClientJsonRpcMessage::notification(ClientNotification::InitializedNotification(
                InitializedNotification::default(),
            )),
            None,
            None,
            HashMap::new(),
        )
        .await;

    server_handle.abort();
    let _ = std::fs::remove_file(&socket_path);
    let _ = std::fs::remove_dir(&dir);

    match result {
        Ok(StreamableHttpPostResponse::Accepted) => {}
        other => panic!("expected Accepted, got: {other:?}"),
    }
    Ok(())
}

/// A non-2xx reply to a notification must stay an error: the 2xx fallback is
/// safe only while it sits below the transport's status check. Runs on Linux
/// CI; the whole file is `cfg(unix)`.
#[tokio::test]
async fn test_unix_socket_notification_server_error_is_rejected() -> anyhow::Result<()> {
    let dir = std::env::temp_dir().join(format!("rmcp-test-unix-error-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let socket_path = dir.join("mcp.sock");
    let _ = std::fs::remove_file(&socket_path);

    async fn error_handler() -> impl IntoResponse {
        (StatusCode::INTERNAL_SERVER_ERROR, "server error")
    }

    let app = Router::new().route("/mcp", post(error_handler));
    let listener = tokio::net::UnixListener::bind(&socket_path)?;
    let server_handle = spawn_unix_server(listener, app);

    let socket_str = socket_path.to_str().unwrap();
    let uri = "http://localhost/mcp";
    let client = UnixSocketHttpClient::new(socket_str, uri);
    let result = client
        .post_message(
            Arc::from(uri),
            ClientJsonRpcMessage::notification(ClientNotification::InitializedNotification(
                InitializedNotification::default(),
            )),
            None,
            None,
            HashMap::new(),
        )
        .await;

    server_handle.abort();
    let _ = std::fs::remove_file(&socket_path);
    let _ = std::fs::remove_dir(&dir);

    match result {
        Err(StreamableHttpError::UnexpectedServerResponse(msg)) => {
            assert!(msg.contains("500"), "expected 500 in error, got: {msg}");
        }
        other => panic!("expected UnexpectedServerResponse(500), got: {other:?}"),
    }
    Ok(())
}
