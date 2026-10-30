//! A 401 or 403 may carry several `WWW-Authenticate` challenges, either in one
//! field or across repeated fields (RFC 9110 §11.6.1). The reqwest client must
//! keep every challenge, in order, so a `Bearer` challenge is not hidden behind
//! another scheme's field, and must read `scope` from the right challenge.
#![cfg(all(
    feature = "transport-streamable-http-client-reqwest",
    not(feature = "local")
))]

use std::{collections::HashMap, sync::Arc};

use axum::{Router, body::Body, http::Response, routing::any};
use http::{HeaderValue, header::WWW_AUTHENTICATE};
use rmcp::{
    model::{ClientJsonRpcMessage, ClientRequest, PingRequest, RequestId},
    transport::streamable_http_client::{StreamableHttpClient, StreamableHttpError},
};
use rstest::rstest;

async fn request(
    post: bool,
    status: u16,
    challenges: Vec<HeaderValue>,
) -> Result<(), StreamableHttpError<reqwest::Error>> {
    let router = Router::new().route(
        "/mcp",
        any(move || {
            let challenges = challenges.clone();
            async move {
                let mut response = Response::builder()
                    .status(status)
                    .body(Body::empty())
                    .unwrap();
                for challenge in challenges {
                    response.headers_mut().append(WWW_AUTHENTICATE, challenge);
                }
                response
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let uri: Arc<str> = format!("http://{}/mcp", listener.local_addr().unwrap()).into();
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let client = reqwest::Client::new();
    let result = if post {
        let message = ClientJsonRpcMessage::request(
            ClientRequest::PingRequest(PingRequest::default()),
            RequestId::Number(1),
        );
        client
            .post_message(uri, message, None, None, HashMap::new())
            .await
            .map(|_| ())
    } else {
        client
            .get_stream(uri, None, None, None, HashMap::new())
            .await
            .map(|_| ())
    };
    server.abort();
    result
}

/// Each case lists the `WWW-Authenticate` fields the server sends and the
/// `scope` a 403 must report. The error must carry all fields joined with
/// `", "`, which RFC 9110 §5.3 defines as equivalent to the separate fields.
#[rstest]
#[case::basic_field_before_bearer_field(
    vec![r#"Basic realm="proxy", scope="basic""#, r#"Bearer resource_metadata="https://example.com/prm", scope="mcp:read""#],
    Some("mcp:read")
)]
#[case::first_bearer_scope_wins(
    vec![r#"Bearer realm="first", scope="read""#, r#"Bearer error="insufficient_scope", scope="write""#],
    Some("read")
)]
#[case::bearer_scope_in_a_later_field(
    vec![r#"Bearer error="invalid_token""#, r#"Bearer realm="second", scope="write""#],
    Some("write")
)]
#[case::semicolon_and_missing_commas(
    vec![r#"Bearer realm="a"; error="insufficient_scope" scope="x""#],
    Some("x")
)]
#[case::quoted_commas(
    vec![r#"Basic realm="proxy, scope=wrong", scope="basic""#, r#"Bearer realm="mcp, api", scope="read,write""#],
    Some("read,write")
)]
#[case::quoted_text_is_not_a_parameter(
    vec![r#"Bearer error_description="scope=wrong, error=wrong", scope="right""#],
    Some("right")
)]
#[case::no_bearer_scope(
    vec![r#"Basic scope="basic""#, r#"Bearer error="insufficient_scope""#],
    None
)]
#[case::basic_only(vec![r#"Basic scope="basic""#], None)]
#[case::several_challenges_in_one_field(
    vec![r#"Basic scope="basic", Bearer scope="read", Bearer scope="write""#],
    Some("read")
)]
#[case::escaped_quotes_and_case_insensitive_names(
    vec![r#"Basic realm="proxy\", scope=wrong", scope="basic""#, r#"bEaReR realm="api\", service", ScOpE = "read\"write""#],
    Some("read\"write")
)]
#[case::unquoted_token(vec![r#"Bearer scope=read:data, error="insufficient_scope""#], Some("read:data"))]
#[case::single(vec![r#"Bearer scope="read""#], Some("read"))]
#[case::bare(vec!["Bearer"], None)]
#[case::empty(vec![""], None)]
#[tokio::test]
async fn retains_every_auth_challenge(
    #[values(false, true)] post: bool,
    #[values(401, 403)] status: u16,
    #[case] challenges: Vec<&str>,
    #[case] scope: Option<&str>,
) {
    let expected = challenges.join(", ");
    let headers = challenges
        .into_iter()
        .map(|value| HeaderValue::from_str(value).unwrap())
        .collect();
    match request(post, status, headers).await.unwrap_err() {
        StreamableHttpError::AuthRequired(error) if status == 401 => {
            assert_eq!(error.www_authenticate_header, expected);
        }
        StreamableHttpError::InsufficientScope(error) if status == 403 => {
            assert_eq!(error.www_authenticate_header, expected);
            assert_eq!(error.required_scope.as_deref(), scope);
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

/// A field value outside visible ASCII cannot be carried in the string-based
/// challenge API; it is still rejected whichever field it occurs in.
#[rstest]
#[tokio::test]
async fn rejects_invalid_auth_header_values(
    #[values(false, true)] post: bool,
    #[values(401, 403)] status: u16,
    #[values(false, true)] valid_first: bool,
) {
    let invalid = HeaderValue::from_bytes(b"\xff").unwrap();
    let headers = if valid_first {
        vec![HeaderValue::from_static("Bearer"), invalid]
    } else {
        vec![invalid]
    };
    assert!(matches!(
        request(post, status, headers).await,
        Err(StreamableHttpError::UnexpectedServerResponse(message))
            if message == "invalid www-authenticate header value"
    ));
}

#[rstest]
#[tokio::test]
async fn missing_auth_headers_are_not_challenges(
    #[values(false, true)] post: bool,
    #[values(401, 403)] status: u16,
) {
    let error = request(post, status, vec![]).await.unwrap_err();
    assert!(!matches!(
        error,
        StreamableHttpError::AuthRequired(_) | StreamableHttpError::InsufficientScope(_)
    ));
}
