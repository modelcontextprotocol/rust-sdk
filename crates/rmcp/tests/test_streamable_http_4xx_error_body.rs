#![cfg(all(
    feature = "transport-streamable-http-client",
    feature = "transport-streamable-http-client-reqwest",
    not(feature = "local")
))]

use std::{collections::HashMap, sync::Arc};

use rmcp::{
    model::{ClientJsonRpcMessage, ClientRequest, PingRequest, RequestId},
    transport::streamable_http_client::{
        StreamableHttpClient, StreamableHttpError, StreamableHttpPostResponse,
    },
};

/// Spin up a minimal axum server that always responds with the given status,
/// content-type, and body — no MCP logic involved.
async fn spawn_mock_server(status: u16, content_type: &'static str, body: &'static str) -> String {
    use axum::{Router, body::Body, http::Response, routing::post};

    let router = Router::new().route(
        "/mcp",
        post(move || async move {
            Response::builder()
                .status(status)
                .header("content-type", content_type)
                .body(Body::from(body))
                .unwrap()
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });

    format!("http://{addr}/mcp")
}

fn ping_message() -> ClientJsonRpcMessage {
    ClientJsonRpcMessage::request(
        ClientRequest::PingRequest(PingRequest::default()),
        RequestId::Number(1),
    )
}

/// HTTP 4xx with Content-Type: application/json and a valid JSON-RPC error body
/// must be surfaced as `StreamableHttpPostResponse::Json`, not swallowed as a
/// transport error.
#[tokio::test]
async fn http_4xx_json_rpc_error_body_is_surfaced_as_json_response() {
    let body = r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32600,"message":"Invalid Request"}}"#;
    let url = spawn_mock_server(400, "application/json", body).await;

    let client = reqwest::Client::new();
    let result = client
        .post_message(
            Arc::from(url.as_str()),
            ping_message(),
            None,
            None,
            HashMap::new(),
        )
        .await;

    match result {
        Ok(StreamableHttpPostResponse::Json(msg, _)) => {
            let json = serde_json::to_value(&msg).unwrap();
            assert_eq!(json["error"]["code"], -32600);
            assert_eq!(json["error"]["message"], "Invalid Request");
        }
        other => panic!("expected Json response, got: {other:?}"),
    }
}

/// HTTP 4xx with non-JSON content-type must still return `UnexpectedServerResponse`
/// (no regression on the original error path).
#[tokio::test]
async fn http_4xx_non_json_body_returns_unexpected_server_response() {
    let url = spawn_mock_server(400, "text/plain", "Bad Request").await;

    let client = reqwest::Client::new();
    let result = client
        .post_message(
            Arc::from(url.as_str()),
            ping_message(),
            None,
            None,
            HashMap::new(),
        )
        .await;

    match result {
        Err(StreamableHttpError::UnexpectedServerResponse(_)) => {}
        other => panic!("expected UnexpectedServerResponse, got: {other:?}"),
    }
}

/// HTTP 4xx with Content-Type: application/json but a body that is NOT a valid
/// JSON-RPC message must fall back to `UnexpectedServerResponse`.
#[tokio::test]
async fn http_4xx_malformed_json_body_falls_back_to_unexpected_server_response() {
    let url = spawn_mock_server(400, "application/json", r#"{"error":"not jsonrpc"}"#).await;

    let client = reqwest::Client::new();
    let result = client
        .post_message(
            Arc::from(url.as_str()),
            ping_message(),
            None,
            None,
            HashMap::new(),
        )
        .await;

    match result {
        Err(StreamableHttpError::UnexpectedServerResponse(_)) => {}
        other => panic!("expected UnexpectedServerResponse, got: {other:?}"),
    }
}
