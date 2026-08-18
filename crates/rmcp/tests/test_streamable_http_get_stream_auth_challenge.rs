#![cfg(all(
    feature = "transport-streamable-http-client",
    feature = "transport-streamable-http-client-reqwest",
    not(feature = "local")
))]

use std::{collections::HashMap, sync::Arc};

use rmcp::transport::streamable_http_client::{StreamableHttpClient, StreamableHttpError};

/// Spin up a minimal axum server whose GET handler always responds with the given
/// status and optional `WWW-Authenticate` header — no MCP logic involved.
async fn spawn_mock_server(status: u16, www_authenticate: Option<&'static str>) -> String {
    use axum::{Router, body::Body, http::Response, routing::get};

    let router = Router::new().route(
        "/mcp",
        get(move || async move {
            let mut builder = Response::builder().status(status);
            if let Some(challenge) = www_authenticate {
                builder = builder.header("www-authenticate", challenge);
            }
            builder.body(Body::empty()).unwrap()
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });

    format!("http://{addr}/mcp")
}

async fn get_stream_against(
    status: u16,
    www_authenticate: Option<&'static str>,
) -> Result<(), StreamableHttpError<reqwest::Error>> {
    let url = spawn_mock_server(status, www_authenticate).await;
    reqwest::Client::new()
        .get_stream(Arc::from(url.as_str()), None, None, None, HashMap::new())
        .await
        .map(|_| ())
}

/// A 401 carrying a `WWW-Authenticate` challenge must surface as `AuthRequired`,
/// which is the variant `AuthClient::call_reacting_to_challenges` catches to
/// refresh the token and retry. Classified as a plain `Client` error instead, the
/// refresh never runs and the stream just fails.
#[tokio::test]
async fn get_stream_maps_401_challenge_to_auth_required() {
    let result = get_stream_against(401, Some("Bearer realm=\"mcp\"")).await;

    match result {
        Err(StreamableHttpError::AuthRequired(err)) => {
            assert_eq!(err.www_authenticate_header, "Bearer realm=\"mcp\"");
        }
        other => panic!("expected AuthRequired, got: {other:?}"),
    }
}

/// A 403 carrying a challenge must surface as `InsufficientScope`, with the
/// required scope extracted from the header.
#[tokio::test]
async fn get_stream_maps_403_challenge_to_insufficient_scope() {
    let result = get_stream_against(
        403,
        Some("Bearer error=\"insufficient_scope\", scope=\"mcp:read\""),
    )
    .await;

    match result {
        Err(StreamableHttpError::InsufficientScope(err)) => {
            assert_eq!(err.required_scope.as_deref(), Some("mcp:read"));
        }
        other => panic!("expected InsufficientScope, got: {other:?}"),
    }
}

/// Without a `WWW-Authenticate` header there is no challenge to act on, so a 401
/// must keep falling through to the ordinary error path rather than being
/// reported as an auth challenge the caller can retry.
#[tokio::test]
async fn get_stream_401_without_challenge_is_not_auth_required() {
    let result = get_stream_against(401, None).await;

    assert!(
        !matches!(result, Err(StreamableHttpError::AuthRequired(_))),
        "401 with no challenge header must not be classified as AuthRequired"
    );
    assert!(result.is_err(), "401 must still be an error");
}

/// 405 keeps its dedicated meaning — the server does not support SSE on GET —
/// and must not be swallowed by the new auth branches.
#[tokio::test]
async fn get_stream_405_still_reports_sse_unsupported() {
    let result = get_stream_against(405, None).await;

    assert!(
        matches!(result, Err(StreamableHttpError::ServerDoesNotSupportSse)),
        "expected ServerDoesNotSupportSse, got: {result:?}"
    );
}
