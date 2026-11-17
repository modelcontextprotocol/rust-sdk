#![cfg(all(
    feature = "client",
    feature = "transport-streamable-http-client",
    feature = "transport-streamable-http-client-reqwest",
    not(feature = "local")
))]

use std::{error::Error, time::Duration};

use axum::{
    Router,
    body::{Body, Bytes},
    http::Response,
    routing::post,
};
use rmcp::{
    ServiceError, ServiceExt,
    model::CallToolRequestParams,
    transport::{
        DynamicTransportError, StreamableHttpClientTransport,
        streamable_http_client::AuthRequiredError,
    },
};
use serde_json::{Value, json};

const CHALLENGE: &str = "Bearer error=\"invalid_token\", error_description=\"revoked\"";

/// Answers the handshake normally, then rejects every later request with a
/// 401 and a challenge, as a server does when a token is revoked mid-session.
async fn spawn_revoking_server() -> String {
    let router = Router::new().route(
        "/mcp",
        post(|body: Bytes| async move {
            let request: Value = serde_json::from_slice(&body).unwrap();
            match request["method"].as_str() {
                Some("initialize") => {
                    let result = json!({
                        "jsonrpc": "2.0",
                        "id": request["id"].clone(),
                        "result": {
                            "protocolVersion": "2025-06-18",
                            "capabilities": { "tools": {} },
                            "serverInfo": { "name": "stand-in", "version": "0" }
                        }
                    });
                    Response::builder()
                        .status(200)
                        .header("content-type", "application/json")
                        .body(Body::from(result.to_string()))
                        .unwrap()
                }
                Some("notifications/initialized") => {
                    Response::builder().status(202).body(Body::empty()).unwrap()
                }
                _ => Response::builder()
                    .status(401)
                    .header("www-authenticate", CHALLENGE)
                    .body(Body::empty())
                    .unwrap(),
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    format!("http://{addr}/mcp")
}

#[tokio::test]
async fn call_tool_401_after_initialize_exposes_the_challenge() {
    let uri = spawn_revoking_server().await;
    let client = tokio::time::timeout(
        Duration::from_secs(5),
        ().serve(StreamableHttpClientTransport::from_uri(uri)),
    )
    .await
    .expect("handshake should not hang")
    .expect("handshake should succeed");

    let error = tokio::time::timeout(
        Duration::from_secs(5),
        client.call_tool(CallToolRequestParams::new("anything")),
    )
    .await
    .expect("call_tool should not hang")
    .expect_err("the server answers 401");

    assert!(
        matches!(error, ServiceError::TransportSend(_)),
        "expected TransportSend, got: {error:?}"
    );
    assert_eq!(error.auth_challenge(), Some(CHALLENGE));
    assert!(error.is_authorization_required());

    // The challenge must also be reachable through the standard error chain.
    let transport_error = error
        .source()
        .expect("TransportSend should have a source")
        .downcast_ref::<DynamicTransportError>()
        .expect("source should be the DynamicTransportError");
    let mut next: Option<&(dyn Error + 'static)> = transport_error.source();
    let mut found = None;
    while let Some(current) = next {
        if let Some(auth) = current.downcast_ref::<AuthRequiredError>() {
            found = Some(auth.www_authenticate_header.clone());
            break;
        }
        next = current.source();
    }
    assert_eq!(found.as_deref(), Some(CHALLENGE));
}

#[test]
fn non_transport_errors_have_no_challenge() {
    let error = ServiceError::TransportClosed;
    assert_eq!(error.auth_challenge(), None);
    assert!(!error.is_authorization_required());
    assert!(error.source().is_none());
}
