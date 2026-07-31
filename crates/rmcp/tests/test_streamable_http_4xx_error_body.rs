#![cfg(all(
    feature = "transport-streamable-http-client",
    feature = "transport-streamable-http-client-reqwest",
    not(feature = "local")
))]

//! Non-2xx POST responses: a JSON-RPC error body must reach the service layer
//! in-band rather than being flattened into a transport error, and the HTTP
//! status must survive alongside it.
//!
//! The status matters because the MCP 2026-07-28
//! [Streamable HTTP backward-compatibility rules][http] key era detection to
//! `400 Bad Request` specifically: a `400` whose body is a recognized modern
//! JSON-RPC error identifies a modern server, while a `400` with an empty or
//! unrecognized body identifies an initialization-era one. Every other status is
//! neither. These responses are therefore reported as
//! `StreamableHttpPostResponse::ErrorResponse`, which keeps the status and the
//! body paired.
//!
//! The in-band delivery contract is unchanged from before that variant existed,
//! and is asserted here through `expect_json`, which is the path the transport
//! worker actually takes.
//!
//! [http]: https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/streamable-http#backward-compatibility

use std::{collections::HashMap, sync::Arc};

use rmcp::{
    model::{ClientJsonRpcMessage, ClientRequest, PingRequest, RequestId, ServerJsonRpcMessage},
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

async fn post_to_mock(
    status: u16,
    content_type: &'static str,
    body: &'static str,
) -> Result<StreamableHttpPostResponse, StreamableHttpError<reqwest::Error>> {
    let url = spawn_mock_server(status, content_type, body).await;
    reqwest::Client::new()
        .post_message(
            Arc::from(url.as_str()),
            ping_message(),
            None,
            None,
            HashMap::new(),
        )
        .await
}

/// A non-2xx with `Content-Type: application/json` and a valid JSON-RPC error
/// body must be surfaced in-band, not swallowed as a transport error, so the
/// service layer can match it to the outstanding request.
#[tokio::test]
async fn http_4xx_json_rpc_error_body_is_surfaced_in_band() {
    let body = r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32600,"message":"Invalid Request"}}"#;
    let result = post_to_mock(400, "application/json", body).await;

    let response = result.expect("a JSON-RPC error body must not become a transport error");
    let message = response
        .expect_json::<reqwest::Error>()
        .expect("the error body must be delivered in-band");
    let json = serde_json::to_value(&message).unwrap();
    assert_eq!(json["error"]["code"], -32600);
    assert_eq!(json["error"]["message"], "Invalid Request");
    assert!(matches!(message, ServerJsonRpcMessage::Error(_)));
}

/// The status is retained next to the parsed body. This is what makes era
/// detection possible: `400` and `500` carrying the same body must not be
/// indistinguishable.
#[tokio::test]
async fn non_2xx_response_preserves_status_alongside_parsed_body() {
    let body = r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"Method not found"}}"#;

    for status in [400u16, 404, 500] {
        let result = post_to_mock(status, "application/json", body).await;

        match result.expect("a JSON-RPC error body must not become a transport error") {
            StreamableHttpPostResponse::ErrorResponse {
                status: got_status,
                jsonrpc_error: Some(ServerJsonRpcMessage::Error(error)),
                ..
            } => {
                assert_eq!(got_status, status, "the HTTP status must be preserved");
                assert_eq!(error.error.code.0, -32601);
            }
            other => panic!("HTTP {status} must preserve its status, got: {other:?}"),
        }
    }
}

/// A non-JSON content type must still yield a transport error: there is no
/// JSON-RPC error to correlate, and the original error path is unchanged.
#[tokio::test]
async fn http_4xx_non_json_body_returns_unexpected_server_response() {
    let result = post_to_mock(400, "text/plain", "Bad Request").await;

    let response = result.expect("a non-2xx is reported as a response, not a send failure");
    let error = response
        .expect_json::<reqwest::Error>()
        .expect_err("a non-JSON body has no JSON-RPC error to surface");
    assert!(matches!(
        error,
        StreamableHttpError::UnexpectedServerResponse(_)
    ));
    let rendered = error.to_string();
    assert!(
        rendered.contains("400") && rendered.contains("Bad Request"),
        "the status and body must survive for diagnostics, got {rendered:?}"
    );
}

/// `application/json` whose body is not a JSON-RPC *error response* must also
/// yield a transport error rather than being misreported as a protocol error.
#[tokio::test]
async fn http_4xx_malformed_json_body_falls_back_to_unexpected_server_response() {
    let result = post_to_mock(400, "application/json", r#"{"error":"not jsonrpc"}"#).await;

    let response = result.expect("a non-2xx is reported as a response, not a send failure");
    let error = response
        .expect_json::<reqwest::Error>()
        .expect_err("a non-JSON-RPC body has no error to surface");
    assert!(matches!(
        error,
        StreamableHttpError::UnexpectedServerResponse(_)
    ));
}

/// An empty `400` body is the spec's canonical initialization-era signal, so it
/// must be reported with its status intact and no parsed error.
#[tokio::test]
async fn http_400_empty_body_is_reported_with_status_and_no_parsed_error() {
    let result = post_to_mock(400, "text/plain", "").await;

    match result.expect("a non-2xx is reported as a response, not a send failure") {
        StreamableHttpPostResponse::ErrorResponse {
            status,
            jsonrpc_error,
            ..
        } => {
            assert_eq!(status, 400);
            assert!(
                jsonrpc_error.is_none(),
                "an empty body carries no JSON-RPC error"
            );
        }
        other => panic!("expected ErrorResponse, got: {other:?}"),
    }
}
