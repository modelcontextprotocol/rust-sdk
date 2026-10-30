use std::{any::TypeId, error::Error};

use rmcp::{
    service::{ClientInitializeError, ServiceError},
    transport::{
        AuthError, DynamicTransportError,
        streamable_http_client::{
            AuthRequiredError, HttpStatusError, InsufficientScopeError, StreamableHttpError,
        },
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

/// A 401 without a `WWW-Authenticate` challenge still says the request lacked
/// valid credentials; discovery then falls back to the well-known URIs. A
/// challenge-less 403 carries no such signal.
#[test]
fn classifies_challenge_less_http_401_only() {
    let unauthorized = TestHttpError::HttpStatus(HttpStatusError::new(401, None));
    let forbidden = TestHttpError::HttpStatus(HttpStatusError::new(403, None));

    assert!(initialization_error(unauthorized).is_authorization_required());
    assert!(!initialization_error(forbidden).is_authorization_required());
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

fn find_http_status<'a>(error: &'a (dyn Error + 'static)) -> &'a HttpStatusError {
    let mut source = Some(error);
    loop {
        let current = source.expect("HTTP status metadata should be an error source");
        if let Some(metadata) = current.downcast_ref::<HttpStatusError>() {
            return metadata;
        }
        source = current.source();
    }
}

/// Status metadata is reachable by walking the initialization error's or the
/// runtime service error's source chain, so callers need not know the
/// concrete transport type.
#[test]
fn http_status_remains_available_as_an_error_source() {
    let status_error = || {
        OuterError(TestHttpError::HttpStatus(HttpStatusError::new(
            503,
            Some("17".to_owned()),
        )))
    };

    let initialization = initialization_error(status_error());
    let metadata = find_http_status(&initialization);
    assert_eq!(metadata.status, 503);
    assert_eq!(metadata.retry_after.as_deref(), Some("17"));

    let runtime = ServiceError::TransportSend(DynamicTransportError::from_parts(
        "test transport",
        TypeId::of::<()>(),
        Box::new(status_error()),
    ));
    let metadata = find_http_status(&runtime);
    assert_eq!(metadata.status, 503);
    assert_eq!(metadata.retry_after.as_deref(), Some("17"));
}
