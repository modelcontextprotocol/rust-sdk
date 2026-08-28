#![cfg(all(
    feature = "transport-streamable-http-client",
    feature = "transport-streamable-http-client-reqwest",
    feature = "client",
    not(feature = "local")
))]

use std::{collections::HashMap, sync::Arc, time::Duration};

use axum::{Router, body::Bytes, http::StatusCode, response::IntoResponse, routing::post};
use rmcp::{
    ServiceError, ServiceExt,
    model::{
        CallToolRequestParams, ClientJsonRpcMessage, ClientNotification, ClientRequest,
        InitializedNotification, PingRequest, RequestId,
    },
    transport::{
        StreamableHttpClientTransport,
        streamable_http_client::{
            StreamableHttpClient, StreamableHttpError, StreamableHttpPostResponse,
        },
    },
};
use rstest::rstest;

/// Initialize succeeds; every later POST (including `tools/call`) returns
/// HTTP 200 + `application/json` with a body that is not a JSON-RPC message.
async fn spawn_malformed_json_request_server(call_body: &'static str) -> String {
    let router = Router::new().route(
        "/mcp",
        post(move |body: Bytes| async move {
            let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
            let method = parsed.get("method").and_then(|m| m.as_str()).unwrap_or("");
            if method == "initialize" {
                let id = parsed.get("id").cloned().unwrap_or(serde_json::json!(1));
                return (
                    StatusCode::OK,
                    [
                        (http::header::CONTENT_TYPE, "application/json"),
                        (
                            http::HeaderName::from_static("mcp-session-id"),
                            "test-session",
                        ),
                    ],
                    serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "protocolVersion": "2025-03-26",
                            "capabilities": { "tools": {} },
                            "serverInfo": { "name": "mock", "version": "0.0.1" }
                        }
                    })
                    .to_string(),
                )
                    .into_response();
            }
            if method == "notifications/initialized" {
                return StatusCode::ACCEPTED.into_response();
            }
            (
                StatusCode::OK,
                [(http::header::CONTENT_TYPE, "application/json")],
                call_body,
            )
                .into_response()
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    format!("http://{addr}/mcp")
}

async fn spawn_json_body_server(body: &'static str) -> String {
    let router = Router::new().route(
        "/mcp",
        post(move || async move {
            (
                StatusCode::OK,
                [(http::header::CONTENT_TYPE, "application/json")],
                body,
            )
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    format!("http://{addr}/mcp")
}

/// Public-API repro: `call_tool` must return a transport error instead of
/// hanging when the server answers a request POST with 200 JSON that is not
/// a JSON-RPC message.
#[rstest]
#[case::empty_object("{}")]
#[case::empty_body("")]
#[case::invalid_json("{")]
#[case::html("<html>not json</html>")]
#[tokio::test]
async fn call_tool_errors_on_malformed_json_200(#[case] call_body: &'static str) {
    let url = spawn_malformed_json_request_server(call_body).await;
    let transport = StreamableHttpClientTransport::from_uri(url);
    let client = ().serve(transport).await.expect("initialize should succeed");
    let peer = client.peer().clone();

    let result = tokio::time::timeout(
        Duration::from_secs(2),
        peer.call_tool(CallToolRequestParams::new("anything")),
    )
    .await
    .expect("call_tool must return instead of hanging on malformed JSON 200");

    match result {
        Err(ServiceError::TransportSend(ref dyn_err)) => {
            let err_msg = format!("{dyn_err}");
            assert!(
                err_msg.contains("unexpected server response"),
                "expected UnexpectedServerResponse, got: {err_msg}"
            );
            let preview = if call_body.is_empty() {
                "<empty>"
            } else {
                call_body
            };
            assert!(
                err_msg.contains(preview),
                "expected body preview {preview:?} in error, got: {err_msg}"
            );
        }
        other => panic!("expected TransportSend(UnexpectedServerResponse), got: {other:?}"),
    }

    let _ = client.cancel().await;
}

/// Direct `post_message` check: a request POST cannot treat a JSON parse
/// failure as Accepted.
#[tokio::test]
async fn post_request_malformed_json_is_unexpected_server_response() {
    let url = spawn_json_body_server("{}").await;
    let client = reqwest::Client::new();
    let result = client
        .post_message(
            Arc::from(url.as_str()),
            ClientJsonRpcMessage::request(
                ClientRequest::PingRequest(PingRequest::default()),
                RequestId::Number(1),
            ),
            None,
            None,
            HashMap::new(),
        )
        .await;

    match result {
        Err(StreamableHttpError::UnexpectedServerResponse(ref msg)) => {
            assert!(
                msg.contains("{}"),
                "expected body preview in error, got: {msg}"
            );
        }
        other => panic!("expected UnexpectedServerResponse, got: {other:?}"),
    }
}

/// Notification POSTs still treat an unusable JSON body as Accepted.
/// That fallback is for messages that do not await a reply.
#[tokio::test]
async fn post_notification_malformed_json_is_still_accepted() {
    let url = spawn_json_body_server("{}").await;
    let client = reqwest::Client::new();
    let result = client
        .post_message(
            Arc::from(url.as_str()),
            ClientJsonRpcMessage::notification(ClientNotification::InitializedNotification(
                InitializedNotification::default(),
            )),
            None,
            None,
            HashMap::new(),
        )
        .await;

    match result {
        Ok(StreamableHttpPostResponse::Accepted) => {}
        other => panic!("expected Accepted, got: {other:?}"),
    }
}
