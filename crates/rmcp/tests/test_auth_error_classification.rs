use std::{
    any::TypeId,
    collections::VecDeque,
    error::Error,
    sync::{Arc, Mutex},
};

use rmcp::{
    service::ClientInitializeError,
    transport::{
        AuthError, DynamicTransportError,
        auth::{
            AuthorizationManager, OAuthHttpClient, OAuthHttpClientError, OAuthHttpClientFuture,
            OAuthHttpRequest, ProtectedResourceMetadataInvalidReason,
        },
        streamable_http_client::{AuthRequiredError, InsufficientScopeError, StreamableHttpError},
    },
};
use thiserror::Error;

type TestHttpError = StreamableHttpError<std::io::Error>;

#[derive(Debug, Error)]
#[error("outer transport wrapper")]
struct OuterError(#[source] TestHttpError);

fn initialization_error(error: impl Error + Send + Sync + 'static) -> ClientInitializeError {
    ClientInitializeError::TransportError {
        error: DynamicTransportError::from_parts(
            "test transport",
            TypeId::of::<()>(),
            Box::new(error),
        ),
        context: "initialize".into(),
    }
}

#[test]
fn classifies_local_authorization_required() {
    let error = TestHttpError::Auth(AuthError::AuthorizationRequired);

    assert!(initialization_error(error).is_authorization_required());
}

#[test]
fn classifies_http_authorization_challenge() {
    let error =
        TestHttpError::AuthRequired(AuthRequiredError::new("Bearer realm=\"mcp\"".to_owned()));

    assert!(initialization_error(error).is_authorization_required());
}

#[test]
fn classifies_authorization_required_through_multiple_sources() {
    let error = OuterError(TestHttpError::Auth(AuthError::AuthorizationRequired));

    assert!(initialization_error(error).is_authorization_required());
}

#[test]
fn does_not_classify_unrelated_transport_errors() {
    let closed = TestHttpError::TransportChannelClosed;
    let refresh = TestHttpError::Auth(AuthError::TokenRefreshFailed("timeout".to_owned()));
    let scope = TestHttpError::InsufficientScope(InsufficientScopeError::new(
        "Bearer error=\"insufficient_scope\"".to_owned(),
        Some("admin".to_owned()),
    ));

    assert!(!initialization_error(closed).is_authorization_required());
    assert!(!initialization_error(refresh).is_authorization_required());
    assert!(!initialization_error(scope).is_authorization_required());
}

#[test]
fn does_not_classify_non_transport_initialization_errors() {
    assert!(!ClientInitializeError::Cancelled.is_authorization_required());
    assert!(
        !ClientInitializeError::ConnectionClosed("server closed the connection".to_owned())
            .is_authorization_required()
    );
}

#[test]
fn http_challenge_remains_available_as_an_error_source() {
    let error =
        TestHttpError::AuthRequired(AuthRequiredError::new("Bearer realm=\"mcp\"".to_owned()));

    let source = error.source().expect("auth challenge should be a source");
    let challenge = source
        .downcast_ref::<AuthRequiredError>()
        .expect("source should retain the challenge type");

    assert_eq!(challenge.www_authenticate_header, "Bearer realm=\"mcp\"");
}

struct DiscoveryHttpClient(Mutex<VecDeque<Result<oauth2::HttpResponse, OAuthHttpClientError>>>);

impl OAuthHttpClient for DiscoveryHttpClient {
    fn execute(&self, _request: OAuthHttpRequest) -> OAuthHttpClientFuture<'_> {
        Box::pin(async {
            self.0
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected discovery request")
        })
    }
}

fn discovery_response(status: u16, body: serde_json::Value) -> oauth2::HttpResponse {
    oauth2::http::Response::builder()
        .status(status)
        .body(serde_json::to_vec(&body).unwrap())
        .unwrap()
}

async fn protected_resource_discovery_error(
    responses: Vec<Result<oauth2::HttpResponse, OAuthHttpClientError>>,
) -> AuthError {
    let client = DiscoveryHttpClient(Mutex::new(responses.into()));
    let manager = AuthorizationManager::new_with_oauth_http_client(
        "https://mcp.example.com/mcp",
        Arc::new(client),
    )
    .await
    .unwrap();

    manager
        .resolve_metadata_from_challenge(Some(
            r#"Bearer resource_metadata="https://mcp.example.com/.well-known/oauth-protected-resource""#,
        ))
        .await
        .unwrap_err()
}

#[derive(Debug, Error)]
#[error("discovery request failed")]
struct DiscoveryRequestError(#[source] std::io::Error);

#[tokio::test]
async fn protected_resource_discovery_request_failure_has_a_structured_error_and_source() {
    let error = protected_resource_discovery_error(vec![Err(Box::new(DiscoveryRequestError(
        std::io::Error::other("connection refused"),
    )))])
    .await;

    assert!(
        matches!(
            &error,
            AuthError::DiscoveryRequestFailed { url, .. }
                if url.as_str() == "https://mcp.example.com/.well-known/oauth-protected-resource"
        ),
        "expected structured discovery request error, got {error:?}"
    );
    let source = error
        .source()
        .expect("discovery request error should retain its source");
    let request_error = source
        .downcast_ref::<DiscoveryRequestError>()
        .expect("source should retain the OAuth HTTP client error type");
    assert_eq!(
        request_error
            .source()
            .expect("nested I/O source should be retained")
            .to_string(),
        "connection refused"
    );
}

#[tokio::test]
async fn protected_resource_invalid_metadata_has_a_structured_error() {
    let error = protected_resource_discovery_error(vec![Ok(discovery_response(
        200,
        serde_json::json!({"authorization_servers": ["https://auth.example.com"]}),
    ))])
    .await;

    assert!(matches!(
        error,
        AuthError::ProtectedResourceMetadataInvalid {
            url,
            reason: ProtectedResourceMetadataInvalidReason::MissingResource
        } if url.as_str() == "https://mcp.example.com/.well-known/oauth-protected-resource"
    ));
}

#[tokio::test]
async fn protected_resource_unavailable_authorization_servers_have_a_structured_error() {
    let error = protected_resource_discovery_error(vec![
        Ok(discovery_response(
            200,
            serde_json::json!({
                "resource": "https://mcp.example.com/mcp",
                "authorization_servers": ["https://auth.example.com"]
            }),
        )),
        Ok(discovery_response(404, serde_json::json!({}))),
        Ok(discovery_response(404, serde_json::json!({}))),
    ])
    .await;

    assert!(matches!(
        error,
        AuthError::AuthorizationServersUnavailable {
            resource_metadata_url,
            authorization_servers
        } if resource_metadata_url.as_str()
                == "https://mcp.example.com/.well-known/oauth-protected-resource"
            && authorization_servers == ["https://auth.example.com"]
    ));
}
