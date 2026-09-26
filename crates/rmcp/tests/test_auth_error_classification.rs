use std::{any::TypeId, error::Error};

use rmcp::{
    service::ClientInitializeError,
    transport::{
        AuthError, DynamicTransportError,
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
