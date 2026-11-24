use std::collections::HashMap;

use http::{HeaderName, HeaderValue, Method};
use tracing::debug;

#[cfg(feature = "auth-dpop")]
use crate::transport::auth::dpop::{DPOP_AUTH_SCHEME, DpopProofFactory};
use crate::transport::{
    auth::{AuthClient, AuthError, IssuedToken},
    streamable_http_client::{
        StreamableHttpClient, StreamableHttpError, StreamableHttpResponseLimits,
    },
};

impl<C> AuthClient<C>
where
    C: StreamableHttpClient + Send + Sync,
{
    /// Run `call` with a token when one is available, reacting to the
    /// server's auth verdict instead of requiring credentials up front:
    ///
    /// - no usable credentials → the request goes out unauthenticated, and a
    ///   401 propagates as [`StreamableHttpError::AuthRequired`] carrying the
    ///   `WWW-Authenticate` challenge for the caller to authorize with;
    /// - a token the server rejects (e.g. revoked) → one silent refresh, one
    ///   retry, then the challenge propagates;
    /// - a refresh that fails for any other reason (credential store, network,
    ///   provider) → that error propagates so the caller can retry instead of
    ///   being sent through a new authorization.
    async fn call_reacting_to_challenges<T, F, Fut>(
        &self,
        method: Method,
        uri: &str,
        auth_token: Option<String>,
        custom_headers: &HashMap<HeaderName, HeaderValue>,
        call: F,
    ) -> Result<T, StreamableHttpError<C::Error>>
    where
        F: Fn(Option<String>, HashMap<HeaderName, HeaderValue>) -> Fut,
        Fut: Future<Output = Result<T, StreamableHttpError<C::Error>>>,
    {
        // Missing credentials are not an error in the reactive model: the
        // request goes out unauthenticated and the server's 401 challenge
        // drives authorization.
        let credential = match auth_token {
            None => {
                let issued = {
                    let manager = self.auth_manager.lock().await;
                    manager.issued_access_token().await
                };
                match issued {
                    Ok(token) => self.credential_for(token).await?,
                    Err(AuthError::AuthorizationRequired) => Credential::None,
                    Err(error) => return Err(error.into()),
                }
            }
            Some(token) => self.credential_for_caller_token(token).await?,
        };
        match self
            .send_authorized(&method, uri, &credential, custom_headers, &call)
            .await
        {
            Err(StreamableHttpError::AuthRequired(challenge)) => {
                // One silent recovery attempt: refresh the rejected token and
                // retry only when the refresh actually produced a new one.
                let Some(sent_token) = credential.secret() else {
                    return Err(StreamableHttpError::AuthRequired(challenge));
                };
                let refreshed = {
                    let manager = self.auth_manager.lock().await;
                    manager.try_refresh_or_reauth().await
                };
                match refreshed {
                    Ok(fresh_token) if fresh_token.secret != sent_token => {
                        let credential = self.credential_for(fresh_token).await?;
                        self.send_authorized(&method, uri, &credential, custom_headers, &call)
                            .await
                    }
                    Ok(_) => Err(StreamableHttpError::AuthRequired(challenge)),
                    // `try_refresh_or_reauth` already reports the cases that need a
                    // new authorization; anything else is retryable or infrastructural.
                    Err(AuthError::AuthorizationRequired) => {
                        debug!("token refresh after server rejection requires authorization");
                        Err(StreamableHttpError::AuthRequired(challenge))
                    }
                    Err(error) => Err(error.into()),
                }
            }
            result => result,
        }
    }

    /// How to present a token the authorization manager issued. A DPoP-bound
    /// token is never sent as Bearer: without a signer this is an error.
    async fn credential_for(&self, token: IssuedToken) -> Result<Credential, AuthError> {
        if !token.dpop_bound {
            return Ok(Credential::Bearer(token.secret));
        }
        #[cfg(feature = "auth-dpop")]
        if let Some(proofs) = self.auth_manager.lock().await.dpop_proof_factory() {
            return Ok(Credential::Dpop(token.secret, proofs.clone()));
        }
        Err(AuthError::InternalError(
            "token is DPoP-bound; set a DpopSigner".to_string(),
        ))
    }

    /// How to present a token the caller passed in.
    ///
    /// Without a DPoP signer, the token is sent as Bearer, as before. With a
    /// signer, the stored credentials decide:
    ///
    /// - the stored token itself → sent with its stored type;
    /// - another token while a DPoP-bound token is stored → refused, because
    ///   it may be an old copy of a DPoP-bound token, which must not be sent
    ///   as Bearer;
    /// - another token otherwise → sent as Bearer.
    async fn credential_for_caller_token(&self, token: String) -> Result<Credential, AuthError> {
        #[cfg(feature = "auth-dpop")]
        {
            let manager = self.auth_manager.lock().await;
            if let Some(proofs) = manager.dpop_proof_factory().cloned() {
                return match manager.stored_access_token().await? {
                    Some(stored) if stored.secret == token && stored.dpop_bound => {
                        Ok(Credential::Dpop(token, proofs))
                    }
                    Some(stored) if stored.secret != token && stored.dpop_bound => {
                        Err(AuthError::InternalError(
                            "a DPoP-bound token is stored; a different caller-supplied token \
                             cannot be sent as Bearer"
                                .to_string(),
                        ))
                    }
                    _ => Ok(Credential::Bearer(token)),
                };
            }
        }
        Ok(Credential::Bearer(token))
    }

    /// Send one request with `credential`.
    ///
    /// A DPoP-bound token goes out as `Authorization: DPoP` with a proof
    /// (SEP-1932). When the server answers 401 with a new `DPoP-Nonce`, the
    /// request is sent once more with a proof that carries it (RFC 9449 §9).
    async fn send_authorized<T, F, Fut>(
        &self,
        method: &Method,
        uri: &str,
        credential: &Credential,
        custom_headers: &HashMap<HeaderName, HeaderValue>,
        call: &F,
    ) -> Result<T, StreamableHttpError<C::Error>>
    where
        F: Fn(Option<String>, HashMap<HeaderName, HeaderValue>) -> Fut,
        Fut: Future<Output = Result<T, StreamableHttpError<C::Error>>>,
    {
        match credential {
            Credential::None => call(None, custom_headers.clone()).await,
            Credential::Bearer(token) => call(Some(token.clone()), custom_headers.clone()).await,
            #[cfg(feature = "auth-dpop")]
            Credential::Dpop(token, proofs) => {
                let (headers, sent_nonce) =
                    dpop_headers(proofs, method, uri, token, custom_headers).await?;
                match call(None, headers).await {
                    Err(StreamableHttpError::AuthRequired(challenge))
                        if challenge.dpop_nonce.as_deref().is_some_and(|nonce| {
                            proofs.record_nonce_for_retry(uri, sent_nonce.as_deref(), nonce)
                        }) =>
                    {
                        debug!("retrying with the server's new DPoP nonce");
                        let (headers, _) =
                            dpop_headers(proofs, method, uri, token, custom_headers).await?;
                        call(None, headers).await
                    }
                    result => result,
                }
            }
        }
    }
}

/// How a request is authorized.
enum Credential {
    /// No token: the request goes out unauthenticated.
    None,
    /// `Authorization: Bearer`.
    Bearer(String),
    /// `Authorization: DPoP` with a proof (SEP-1932).
    #[cfg(feature = "auth-dpop")]
    Dpop(String, DpopProofFactory),
}

impl Credential {
    fn secret(&self) -> Option<&str> {
        match self {
            Credential::None => None,
            Credential::Bearer(token) => Some(token),
            #[cfg(feature = "auth-dpop")]
            Credential::Dpop(token, _) => Some(token),
        }
    }
}

#[cfg(feature = "auth-dpop")]
async fn dpop_headers<E: std::error::Error + Send + Sync + 'static>(
    proofs: &DpopProofFactory,
    method: &Method,
    uri: &str,
    token: &str,
    custom_headers: &HashMap<HeaderName, HeaderValue>,
) -> Result<(HashMap<HeaderName, HeaderValue>, Option<String>), StreamableHttpError<E>> {
    let (proof, nonce) = proofs
        .proof_and_nonce(method.as_str(), uri, Some(token))
        .await
        .map_err(|error| StreamableHttpError::Auth(AuthError::InternalError(error.to_string())))?;
    let invalid = |_| {
        StreamableHttpError::Auth(AuthError::InternalError(
            "DPoP header value is not valid".to_string(),
        ))
    };
    let mut headers = custom_headers.clone();
    let mut authorization =
        HeaderValue::try_from(format!("{DPOP_AUTH_SCHEME} {token}")).map_err(invalid)?;
    authorization.set_sensitive(true);
    let mut proof = HeaderValue::try_from(proof).map_err(invalid)?;
    proof.set_sensitive(true);
    headers.insert(http::header::AUTHORIZATION, authorization);
    headers.insert(HeaderName::from_static("dpop"), proof);
    Ok((headers, nonce))
}

impl<C> StreamableHttpClient for AuthClient<C>
where
    C: StreamableHttpClient + Send + Sync,
{
    type Error = C::Error;

    async fn delete_session(
        &self,
        uri: std::sync::Arc<str>,
        session_id: std::sync::Arc<str>,
        auth_token: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<(), crate::transport::streamable_http_client::StreamableHttpError<Self::Error>>
    {
        self.call_reacting_to_challenges(
            Method::DELETE,
            &uri,
            auth_token,
            &custom_headers,
            |token, custom_headers| {
                let uri = uri.clone();
                let session_id = session_id.clone();
                async move {
                    self.http_client
                        .delete_session(uri, session_id, token, custom_headers)
                        .await
                }
            },
        )
        .await
    }

    async fn get_stream(
        &self,
        uri: std::sync::Arc<str>,
        session_id: Option<std::sync::Arc<str>>,
        last_event_id: Option<String>,
        auth_token: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<
        futures::stream::BoxStream<'static, Result<sse_stream::Sse, sse_stream::Error>>,
        crate::transport::streamable_http_client::StreamableHttpError<Self::Error>,
    > {
        self.call_reacting_to_challenges(
            Method::GET,
            &uri,
            auth_token,
            &custom_headers,
            |token, custom_headers| {
                let uri = uri.clone();
                let session_id = session_id.clone();
                let last_event_id = last_event_id.clone();
                async move {
                    self.http_client
                        .get_stream(uri, session_id, last_event_id, token, custom_headers)
                        .await
                }
            },
        )
        .await
    }

    async fn get_stream_with_max_sse_event_size(
        &self,
        uri: std::sync::Arc<str>,
        session_id: Option<std::sync::Arc<str>>,
        last_event_id: Option<String>,
        auth_token: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
        max_sse_event_size: usize,
    ) -> Result<
        futures::stream::BoxStream<'static, Result<sse_stream::Sse, sse_stream::Error>>,
        crate::transport::streamable_http_client::StreamableHttpError<Self::Error>,
    > {
        self.call_reacting_to_challenges(
            Method::GET,
            &uri,
            auth_token,
            &custom_headers,
            |token, custom_headers| {
                let uri = uri.clone();
                let session_id = session_id.clone();
                let last_event_id = last_event_id.clone();
                async move {
                    self.http_client
                        .get_stream_with_max_sse_event_size(
                            uri,
                            session_id,
                            last_event_id,
                            token,
                            custom_headers,
                            max_sse_event_size,
                        )
                        .await
                }
            },
        )
        .await
    }

    async fn post_message(
        &self,
        uri: std::sync::Arc<str>,
        message: crate::model::ClientJsonRpcMessage,
        session_id: Option<std::sync::Arc<str>>,
        auth_token: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<
        crate::transport::streamable_http_client::StreamableHttpPostResponse,
        StreamableHttpError<Self::Error>,
    > {
        self.call_reacting_to_challenges(
            Method::POST,
            &uri,
            auth_token,
            &custom_headers,
            |token, custom_headers| {
                let uri = uri.clone();
                let message = message.clone();
                let session_id = session_id.clone();
                async move {
                    self.http_client
                        .post_message(uri, message, session_id, token, custom_headers)
                        .await
                }
            },
        )
        .await
    }

    async fn post_message_with_max_sse_event_size(
        &self,
        uri: std::sync::Arc<str>,
        message: crate::model::ClientJsonRpcMessage,
        session_id: Option<std::sync::Arc<str>>,
        auth_token: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
        max_sse_event_size: usize,
    ) -> Result<
        crate::transport::streamable_http_client::StreamableHttpPostResponse,
        StreamableHttpError<Self::Error>,
    > {
        self.call_reacting_to_challenges(
            Method::POST,
            &uri,
            auth_token,
            &custom_headers,
            |token, custom_headers| {
                let uri = uri.clone();
                let message = message.clone();
                let session_id = session_id.clone();
                async move {
                    self.http_client
                        .post_message_with_max_sse_event_size(
                            uri,
                            message,
                            session_id,
                            token,
                            custom_headers,
                            max_sse_event_size,
                        )
                        .await
                }
            },
        )
        .await
    }

    async fn post_message_with_response_limits(
        &self,
        uri: std::sync::Arc<str>,
        message: crate::model::ClientJsonRpcMessage,
        session_id: Option<std::sync::Arc<str>>,
        auth_token: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
        limits: StreamableHttpResponseLimits,
    ) -> Result<
        crate::transport::streamable_http_client::StreamableHttpPostResponse,
        StreamableHttpError<Self::Error>,
    > {
        self.call_reacting_to_challenges(
            Method::POST,
            &uri,
            auth_token,
            &custom_headers,
            |token, custom_headers| {
                let uri = uri.clone();
                let message = message.clone();
                let session_id = session_id.clone();
                async move {
                    self.http_client
                        .post_message_with_response_limits(
                            uri,
                            message,
                            session_id,
                            token,
                            custom_headers,
                            limits,
                        )
                        .await
                }
            },
        )
        .await
    }
}

#[cfg(all(test, feature = "transport-streamable-http-client-reqwest"))]
mod tests {
    use std::sync::{Arc, Mutex};

    use oauth2::{AccessToken, RefreshToken, basic::BasicTokenType};

    use super::*;
    use crate::transport::{
        auth::{
            AuthorizationManager, AuthorizationMetadata, CredentialRefreshGuard, CredentialStore,
            InMemoryCredentialStore, OAuthHttpClient, OAuthHttpClientFuture, OAuthHttpRequest,
            OAuthTokenResponse, StoredCredentials, VendorExtraTokenFields,
        },
        streamable_http_client::{AuthRequiredError, StreamableHttpPostResponse},
    };

    struct RefreshClient;

    impl OAuthHttpClient for RefreshClient {
        fn execute(&self, request: OAuthHttpRequest) -> OAuthHttpClientFuture<'_> {
            Box::pin(async move {
                assert_eq!(request.request.uri(), "https://auth.example.com/token");
                assert_eq!(request.request.method(), http::Method::POST);
                Ok(oauth2::http::Response::builder()
                    .status(200)
                    .header("content-type", "application/json")
                    .body(br#"{"access_token":"fresh-token","token_type":"Bearer"}"#.to_vec())
                    .unwrap())
            })
        }
    }

    type RecordedPostCall = (Option<String>, StreamableHttpResponseLimits);

    #[derive(Clone, Default)]
    struct RecordingLimitsClient {
        calls: Arc<Mutex<Vec<RecordedPostCall>>>,
    }

    impl StreamableHttpClient for RecordingLimitsClient {
        type Error = std::io::Error;

        async fn post_message(
            &self,
            _: Arc<str>,
            _: crate::model::ClientJsonRpcMessage,
            _: Option<Arc<str>>,
            _: Option<String>,
            _: HashMap<HeaderName, HeaderValue>,
        ) -> Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>> {
            panic!("response limits must not use the legacy POST method");
        }

        async fn delete_session(
            &self,
            _: Arc<str>,
            _: Arc<str>,
            _: Option<String>,
            _: HashMap<HeaderName, HeaderValue>,
        ) -> Result<(), StreamableHttpError<Self::Error>> {
            unreachable!("this test only sends POST requests")
        }

        async fn get_stream(
            &self,
            _: Arc<str>,
            _: Option<Arc<str>>,
            _: Option<String>,
            _: Option<String>,
            _: HashMap<HeaderName, HeaderValue>,
        ) -> Result<
            futures::stream::BoxStream<'static, Result<sse_stream::Sse, sse_stream::Error>>,
            StreamableHttpError<Self::Error>,
        > {
            unreachable!("this test only sends POST requests")
        }

        async fn post_message_with_response_limits(
            &self,
            uri: Arc<str>,
            message: crate::model::ClientJsonRpcMessage,
            session_id: Option<Arc<str>>,
            auth_token: Option<String>,
            custom_headers: HashMap<HeaderName, HeaderValue>,
            limits: StreamableHttpResponseLimits,
        ) -> Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>> {
            assert_eq!(uri.as_ref(), "https://mcp.example.com/mcp");
            assert_eq!(session_id.as_deref(), Some("test-session"));
            assert_eq!(serde_json::to_value(message).unwrap()["method"], "ping");
            assert_eq!(
                custom_headers.get(&HeaderName::from_static("x-test-header")),
                Some(&HeaderValue::from_static("preserved")),
            );
            let mut calls = self.calls.lock().unwrap();
            calls.push((auth_token, limits));
            if calls.len() == 1 {
                Err(StreamableHttpError::AuthRequired(AuthRequiredError::new(
                    "Bearer".into(),
                )))
            } else {
                Err(StreamableHttpError::ResponseBodyTooLarge {
                    max_size: limits.max_json_response_size,
                })
            }
        }
    }

    #[tokio::test]
    async fn response_limits_survive_auth_refresh_and_size_errors_do_not_retry() {
        let mut manager = AuthorizationManager::new_with_oauth_http_client(
            "https://mcp.example.com/mcp",
            Arc::new(RefreshClient),
        )
        .await
        .unwrap();
        manager.set_metadata(AuthorizationMetadata {
            authorization_endpoint: "https://auth.example.com/authorize".into(),
            token_endpoint: "https://auth.example.com/token".into(),
            ..Default::default()
        });
        manager.configure_client_id("client").unwrap();
        let store = InMemoryCredentialStore::new();
        let credentials = serde_json::from_value(serde_json::json!({
            "access_token": "old-token",
            "refresh_token": "refresh-token",
            "token_type": "Bearer",
        }))
        .unwrap();
        store
            .save(StoredCredentials::new(
                "client".into(),
                Some(credentials),
                vec![],
                None,
            ))
            .await
            .unwrap();
        manager.set_credential_store(store);
        let recording = RecordingLimitsClient::default();
        let client = AuthClient::new(recording.clone(), manager);
        let mut headers = HashMap::new();
        headers.insert(
            HeaderName::from_static("x-test-header"),
            HeaderValue::from_static("preserved"),
        );
        let limits = StreamableHttpResponseLimits {
            max_sse_event_size: 17,
            max_json_response_size: 31,
            max_error_response_size: 13,
        };
        let result = client
            .post_message_with_response_limits(
                "https://mcp.example.com/mcp".into(),
                serde_json::from_value(
                    serde_json::json!({"jsonrpc":"2.0", "id":1, "method":"ping"}),
                )
                .unwrap(),
                Some("test-session".into()),
                None,
                headers,
                limits,
            )
            .await;
        assert!(matches!(
            result,
            Err(StreamableHttpError::ResponseBodyTooLarge { max_size: 31 })
        ));
        let calls = recording.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].0.as_deref(), Some("old-token"));
        assert_eq!(calls[1].0.as_deref(), Some("fresh-token"));
        for (_, observed) in calls.iter() {
            assert_eq!(observed.max_sse_event_size, 17);
            assert_eq!(observed.max_json_response_size, 31);
            assert_eq!(observed.max_error_response_size, 13);
        }
    }

    struct UnavailableStore;

    #[async_trait::async_trait]
    impl CredentialStore for UnavailableStore {
        async fn load(&self) -> Result<Option<StoredCredentials>, AuthError> {
            unreachable!("guard failure must stop the credential load")
        }

        async fn save(&self, _: StoredCredentials) -> Result<(), AuthError> {
            unreachable!("guard failure must stop the credential save")
        }

        async fn clear(&self) -> Result<(), AuthError> {
            unreachable!("refresh must not clear credentials")
        }

        async fn acquire_refresh_guard(&self) -> Result<Option<CredentialRefreshGuard>, AuthError> {
            Err(AuthError::CredentialStoreError("guard unavailable".into()))
        }
    }

    #[tokio::test]
    async fn reactive_refresh_preserves_credential_store_failure() {
        let mut manager = AuthorizationManager::new("https://mcp.example.com/mcp")
            .await
            .unwrap();
        manager.set_metadata(AuthorizationMetadata {
            authorization_endpoint: "https://auth.example.com/authorize".into(),
            token_endpoint: "https://auth.example.com/token".into(),
            ..Default::default()
        });
        manager.configure_client_id("client").unwrap();
        manager.set_credential_store(UnavailableStore);
        let client = AuthClient::new(reqwest::Client::new(), manager);

        let error = client
            .call_reacting_to_challenges(
                Method::POST,
                "https://mcp.example.com/mcp",
                Some("old-token".into()),
                &HashMap::new(),
                |_, _| async {
                    Err::<(), _>(StreamableHttpError::AuthRequired(AuthRequiredError::new(
                        "Bearer".into(),
                    )))
                },
            )
            .await
            .unwrap_err();

        assert!(matches!(error,
            StreamableHttpError::Auth(AuthError::CredentialStoreError(message))
                if message == "guard unavailable"));
    }

    struct UnreachableTokenEndpoint;

    impl OAuthHttpClient for UnreachableTokenEndpoint {
        fn execute(&self, _: OAuthHttpRequest) -> OAuthHttpClientFuture<'_> {
            Box::pin(async { Err("token endpoint unreachable".into()) })
        }
    }

    struct RejectingTokenEndpoint;

    impl OAuthHttpClient for RejectingTokenEndpoint {
        fn execute(&self, _: OAuthHttpRequest) -> OAuthHttpClientFuture<'_> {
            Box::pin(async {
                Ok(oauth2::http::Response::builder()
                    .status(400)
                    .header("content-type", "application/json")
                    .body(br#"{"error":"invalid_grant"}"#.to_vec())
                    .unwrap())
            })
        }
    }

    /// A manager holding a refresh token the given token endpoint will answer for.
    async fn manager_with_stored_refresh_token(
        token_endpoint: Arc<dyn OAuthHttpClient>,
    ) -> AuthorizationManager {
        let mut manager = AuthorizationManager::new_with_oauth_http_client(
            "https://mcp.example.com/mcp",
            token_endpoint,
        )
        .await
        .unwrap();
        manager.set_metadata(AuthorizationMetadata {
            authorization_endpoint: "https://auth.example.com/authorize".into(),
            token_endpoint: "https://auth.example.com/token".into(),
            ..Default::default()
        });
        manager.configure_client_id("client").unwrap();

        let mut token_response = OAuthTokenResponse::new(
            AccessToken::new("old-token".into()),
            BasicTokenType::Bearer,
            VendorExtraTokenFields::default(),
        );
        token_response.set_refresh_token(Some(RefreshToken::new("stored-refresh".into())));
        let store = InMemoryCredentialStore::new();
        store
            .save(StoredCredentials::new(
                "client".into(),
                Some(token_response),
                vec![],
                None,
            ))
            .await
            .unwrap();
        manager.set_credential_store(store);
        manager
    }

    /// Drive one call whose server answer is a 401 challenge.
    async fn challenge_once(manager: AuthorizationManager) -> StreamableHttpError<reqwest::Error> {
        AuthClient::new(reqwest::Client::new(), manager)
            .call_reacting_to_challenges(
                Method::POST,
                "https://mcp.example.com/mcp",
                Some("old-token".into()),
                &HashMap::new(),
                |_, _| async {
                    Err::<(), _>(StreamableHttpError::AuthRequired(AuthRequiredError::new(
                        "Bearer".into(),
                    )))
                },
            )
            .await
            .unwrap_err()
    }

    #[tokio::test]
    async fn reactive_refresh_propagates_retryable_refresh_failure() {
        let manager = manager_with_stored_refresh_token(Arc::new(UnreachableTokenEndpoint)).await;

        let error = challenge_once(manager).await;

        assert!(
            matches!(
                error,
                StreamableHttpError::Auth(AuthError::TokenRefreshFailed(_))
            ),
            "a retryable refresh failure must reach the caller instead of asking for a new authorization, got: {error:?}"
        );
    }

    #[tokio::test]
    async fn reactive_refresh_reports_a_rejected_refresh_token_as_a_challenge() {
        let manager = manager_with_stored_refresh_token(Arc::new(RejectingTokenEndpoint)).await;

        let error = challenge_once(manager).await;

        assert!(
            matches!(error, StreamableHttpError::AuthRequired(_)),
            "a definitively rejected refresh token must surface the challenge, got: {error:?}"
        );
    }
}

#[cfg(all(test, feature = "auth-dpop"))]
mod dpop_tests {
    use std::sync::{Arc, Mutex};

    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    use oauth2::{AccessToken, RefreshToken, basic::BasicTokenType};

    use super::*;
    use crate::transport::{
        auth::{
            AuthorizationManager, AuthorizationMetadata, CredentialStore, InMemoryCredentialStore,
            OAuthHttpClient, OAuthHttpClientFuture, OAuthHttpRequest, OAuthTokenResponse,
            StoredCredentials, VendorExtraTokenFields,
            dpop::{Es256DpopSigner, access_token_hash},
        },
        streamable_http_client::AuthRequiredError,
    };

    fn proof_claims(proof: &str) -> serde_json::Value {
        let claims = proof.split('.').nth(1).expect("proof has claims");
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(claims).unwrap()).unwrap()
    }

    /// Answers the first token request with `use_dpop_nonce` and the second
    /// with a DPoP-bound token.
    #[derive(Default)]
    struct NonceTokenEndpoint {
        proofs: Mutex<Vec<String>>,
    }

    impl OAuthHttpClient for NonceTokenEndpoint {
        fn execute(&self, request: OAuthHttpRequest) -> OAuthHttpClientFuture<'_> {
            Box::pin(async move {
                let proof = request
                    .request
                    .headers()
                    .get("dpop")
                    .expect("token request carries a DPoP proof")
                    .to_str()
                    .unwrap()
                    .to_string();
                let mut proofs = self.proofs.lock().unwrap();
                proofs.push(proof);
                let response = if proofs.len() == 1 {
                    oauth2::http::Response::builder()
                        .status(400)
                        .header("content-type", "application/json")
                        .header("DPoP-Nonce", "as-nonce")
                        .body(br#"{"error":"use_dpop_nonce"}"#.to_vec())
                } else {
                    oauth2::http::Response::builder()
                        .status(200)
                        .header("content-type", "application/json")
                        .body(
                            br#"{"access_token":"fresh-token","token_type":"DPoP","refresh_token":"r2"}"#
                                .to_vec(),
                        )
                };
                Ok(response.unwrap())
            })
        }
    }

    async fn manager_with_token(
        token_endpoint: Arc<dyn OAuthHttpClient>,
        token_type: BasicTokenType,
    ) -> AuthorizationManager {
        let mut manager = manager_without_signer(token_endpoint, token_type).await;
        manager.set_dpop_signer(Arc::new(Es256DpopSigner::generate()));
        manager
    }

    async fn manager_without_signer(
        token_endpoint: Arc<dyn OAuthHttpClient>,
        token_type: BasicTokenType,
    ) -> AuthorizationManager {
        let mut manager = AuthorizationManager::new_with_oauth_http_client(
            "https://mcp.example.com/mcp",
            token_endpoint,
        )
        .await
        .unwrap();
        manager.set_metadata(AuthorizationMetadata {
            authorization_endpoint: "https://auth.example.com/authorize".into(),
            token_endpoint: "https://auth.example.com/token".into(),
            ..Default::default()
        });
        manager.configure_client_id("client").unwrap();
        let mut token_response = OAuthTokenResponse::new(
            AccessToken::new("old-token".into()),
            token_type,
            VendorExtraTokenFields::default(),
        );
        token_response.set_refresh_token(Some(RefreshToken::new("stored-refresh".into())));
        let store = InMemoryCredentialStore::new();
        store
            .save(StoredCredentials::new(
                "client".into(),
                Some(token_response),
                vec![],
                None,
            ))
            .await
            .unwrap();
        manager.set_credential_store(store);
        manager
    }

    type Recorded = Arc<Mutex<Vec<(Option<String>, HashMap<HeaderName, HeaderValue>)>>>;

    fn header(headers: &HashMap<HeaderName, HeaderValue>, name: &str) -> Option<String> {
        headers
            .get(&HeaderName::from_bytes(name.as_bytes()).unwrap())
            .map(|value| value.to_str().unwrap().to_string())
    }

    #[tokio::test]
    async fn dpop_token_gets_nonce_retry_then_refresh_with_as_nonce() {
        let token_endpoint = Arc::new(NonceTokenEndpoint::default());
        let manager = manager_with_token(
            token_endpoint.clone(),
            BasicTokenType::Extension("DPoP".into()),
        )
        .await;
        let client = AuthClient::new(reqwest::Client::new(), manager);
        let recorded: Recorded = Arc::default();

        let mut custom = HashMap::new();
        custom.insert(
            HeaderName::from_static("x-test"),
            HeaderValue::from_static("kept"),
        );
        let result = client
            .call_reacting_to_challenges(
                Method::POST,
                "https://mcp.example.com/mcp?ignored=1",
                None,
                &custom,
                |token, headers| {
                    let recorded = recorded.clone();
                    async move {
                        let mut calls = recorded.lock().unwrap();
                        calls.push((token, headers));
                        match calls.len() {
                            // The first proof has no nonce: ask for one.
                            1 => Err(StreamableHttpError::<reqwest::Error>::AuthRequired(
                                AuthRequiredError::new(
                                    r#"DPoP error="use_dpop_nonce", algs="ES256""#.into(),
                                )
                                .with_dpop_nonce("rs-nonce"),
                            )),
                            // The same nonce again: the token itself is rejected.
                            2 => Err(StreamableHttpError::AuthRequired(
                                AuthRequiredError::new(r#"DPoP error="invalid_token""#.into())
                                    .with_dpop_nonce("rs-nonce"),
                            )),
                            _ => Ok(()),
                        }
                    }
                },
            )
            .await;
        assert!(result.is_ok(), "{result:?}");

        let calls = recorded.lock().unwrap();
        assert_eq!(calls.len(), 3);
        for (index, (token, headers)) in calls.iter().enumerate() {
            let expected_token = if index < 2 {
                "old-token"
            } else {
                "fresh-token"
            };
            assert!(token.is_none(), "DPoP requests must not also use Bearer");
            assert_eq!(header(headers, "x-test").as_deref(), Some("kept"));
            assert_eq!(
                header(headers, "authorization"),
                Some(format!("DPoP {expected_token}"))
            );
            let claims = proof_claims(&header(headers, "dpop").expect("DPoP proof"));
            assert_eq!(claims["htm"], "POST");
            assert_eq!(claims["htu"], "https://mcp.example.com/mcp");
            assert_eq!(claims["ath"], access_token_hash(expected_token));
            if index == 0 {
                assert!(claims.get("nonce").is_none());
            } else {
                assert_eq!(claims["nonce"], "rs-nonce");
            }
        }

        let token_proofs = token_endpoint.proofs.lock().unwrap();
        assert_eq!(token_proofs.len(), 2);
        let first = proof_claims(&token_proofs[0]);
        let second = proof_claims(&token_proofs[1]);
        assert_eq!(first["htu"], "https://auth.example.com/token");
        assert!(first.get("ath").is_none());
        assert!(first.get("nonce").is_none());
        assert_eq!(second["nonce"], "as-nonce");
        assert_ne!(first["jti"], second["jti"]);
    }

    #[tokio::test]
    async fn bearer_token_is_sent_as_bearer_even_with_a_signer() {
        let manager = manager_with_token(
            Arc::new(NonceTokenEndpoint::default()),
            BasicTokenType::Bearer,
        )
        .await;
        let client = AuthClient::new(reqwest::Client::new(), manager);
        let (result, calls) =
            record_calls(&client, Method::GET, "https://mcp.example.com/mcp").await;
        result.unwrap();
        assert_eq!(calls[0].0.as_deref(), Some("old-token"));
        assert!(header(&calls[0].1, "dpop").is_none());
        assert!(header(&calls[0].1, "authorization").is_none());
    }

    async fn record_calls(
        client: &AuthClient<reqwest::Client>,
        method: Method,
        uri: &str,
    ) -> (
        Result<(), StreamableHttpError<reqwest::Error>>,
        Vec<(Option<String>, HashMap<HeaderName, HeaderValue>)>,
    ) {
        let recorded: Recorded = Arc::default();
        let result = client
            .call_reacting_to_challenges(method, uri, None, &HashMap::new(), |token, headers| {
                let recorded = recorded.clone();
                async move {
                    recorded.lock().unwrap().push((token, headers));
                    Ok::<(), StreamableHttpError<reqwest::Error>>(())
                }
            })
            .await;
        let calls = recorded.lock().unwrap().clone();
        (result, calls)
    }

    /// Send one request with a caller-supplied token and record what went out.
    async fn record_caller_token_calls(
        client: &AuthClient<reqwest::Client>,
        token: &str,
    ) -> (
        Result<(), StreamableHttpError<reqwest::Error>>,
        Vec<(Option<String>, HashMap<HeaderName, HeaderValue>)>,
    ) {
        let recorded: Recorded = Arc::default();
        let result = client
            .call_reacting_to_challenges(
                Method::GET,
                "https://mcp.example.com/mcp",
                Some(token.into()),
                &HashMap::new(),
                |token, headers| {
                    let recorded = recorded.clone();
                    async move {
                        recorded.lock().unwrap().push((token, headers));
                        Ok::<(), StreamableHttpError<reqwest::Error>>(())
                    }
                },
            )
            .await;
        let calls = recorded.lock().unwrap().clone();
        (result, calls)
    }

    #[tokio::test]
    async fn caller_token_matching_the_stored_dpop_token_is_sent_as_dpop() {
        let manager = manager_with_token(
            Arc::new(NonceTokenEndpoint::default()),
            BasicTokenType::Extension("DPoP".into()),
        )
        .await;
        let client = AuthClient::new(reqwest::Client::new(), manager);

        let (result, calls) = record_caller_token_calls(&client, "old-token").await;

        result.unwrap();
        assert_eq!(calls.len(), 1);
        assert!(calls[0].0.is_none());
        assert_eq!(
            header(&calls[0].1, "authorization").as_deref(),
            Some("DPoP old-token")
        );
        assert!(header(&calls[0].1, "dpop").is_some());
    }

    #[tokio::test]
    async fn other_caller_token_is_refused_while_a_dpop_token_is_stored() {
        let manager = manager_with_token(
            Arc::new(NonceTokenEndpoint::default()),
            BasicTokenType::Extension("DPoP".into()),
        )
        .await;
        let client = AuthClient::new(reqwest::Client::new(), manager);

        // For example, an old copy of a DPoP-bound token that has since been
        // refreshed. It must not go out as Bearer.
        let (result, calls) = record_caller_token_calls(&client, "stale-token").await;

        assert!(
            matches!(
                &result,
                Err(StreamableHttpError::Auth(AuthError::InternalError(message)))
                    if message.contains("caller-supplied token cannot be sent as Bearer")
            ),
            "{result:?}"
        );
        assert!(calls.is_empty(), "nothing may be sent");
    }

    #[tokio::test]
    async fn other_caller_token_is_sent_as_bearer_when_the_stored_token_is_bearer() {
        let manager = manager_with_token(
            Arc::new(NonceTokenEndpoint::default()),
            BasicTokenType::Bearer,
        )
        .await;
        let client = AuthClient::new(reqwest::Client::new(), manager);

        let (result, calls) = record_caller_token_calls(&client, "other-token").await;

        result.unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0.as_deref(), Some("other-token"));
        assert!(header(&calls[0].1, "dpop").is_none());
        assert!(header(&calls[0].1, "authorization").is_none());
    }

    #[tokio::test]
    async fn token_type_is_matched_case_insensitively() {
        let manager = manager_with_token(
            Arc::new(NonceTokenEndpoint::default()),
            BasicTokenType::Extension("dpop".into()),
        )
        .await;
        let client = AuthClient::new(reqwest::Client::new(), manager);

        let (result, calls) =
            record_calls(&client, Method::GET, "https://mcp.example.com/mcp").await;

        result.unwrap();
        assert!(calls[0].0.is_none());
        assert_eq!(
            header(&calls[0].1, "authorization").as_deref(),
            Some("DPoP old-token")
        );
        assert!(header(&calls[0].1, "dpop").is_some());
        for name in [http::header::AUTHORIZATION, HeaderName::from_static("dpop")] {
            assert!(calls[0].1[&name].is_sensitive(), "{name} must be redacted");
        }
    }

    #[tokio::test]
    async fn dpop_bound_token_without_a_signer_is_not_sent_as_bearer() {
        let manager = manager_without_signer(
            Arc::new(NonceTokenEndpoint::default()),
            BasicTokenType::Extension("DPoP".into()),
        )
        .await;
        let client = AuthClient::new(reqwest::Client::new(), manager);

        let (result, calls) =
            record_calls(&client, Method::GET, "https://mcp.example.com/mcp").await;

        assert!(
            matches!(
                &result,
                Err(StreamableHttpError::Auth(AuthError::InternalError(message)))
                    if message.contains("DpopSigner")
            ),
            "{result:?}"
        );
        assert!(calls.is_empty(), "nothing may be sent");
    }

    struct FailingStore;

    #[async_trait::async_trait]
    impl CredentialStore for FailingStore {
        async fn load(&self) -> Result<Option<StoredCredentials>, AuthError> {
            Err(AuthError::CredentialStoreError("store offline".into()))
        }

        async fn save(&self, _: StoredCredentials) -> Result<(), AuthError> {
            Ok(())
        }

        async fn clear(&self) -> Result<(), AuthError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn credential_store_error_is_surfaced_instead_of_sending_bearer() {
        let mut manager = manager_with_token(
            Arc::new(NonceTokenEndpoint::default()),
            BasicTokenType::Extension("DPoP".into()),
        )
        .await;
        manager.set_credential_store(FailingStore);
        let client = AuthClient::new(reqwest::Client::new(), manager);

        let (result, calls) = record_caller_token_calls(&client, "old-token").await;

        assert!(
            matches!(
                &result,
                Err(StreamableHttpError::Auth(AuthError::CredentialStoreError(
                    _
                )))
            ),
            "{result:?}"
        );
        assert!(calls.is_empty());
    }

    #[tokio::test]
    async fn nonce_already_stored_by_a_concurrent_request_still_gets_a_retry() {
        let manager = manager_with_token(
            Arc::new(NonceTokenEndpoint::default()),
            BasicTokenType::Extension("DPoP".into()),
        )
        .await;
        let factory = manager.dpop_proof_factory().unwrap().clone();
        let client = AuthClient::new(reqwest::Client::new(), manager);
        let recorded: Recorded = Arc::default();
        let uri = "https://mcp.example.com/mcp";

        let result = client
            .call_reacting_to_challenges(
                Method::POST,
                uri,
                None,
                &HashMap::new(),
                |token, headers| {
                    let recorded = recorded.clone();
                    let factory = factory.clone();
                    async move {
                        let mut calls = recorded.lock().unwrap();
                        calls.push((token, headers));
                        if calls.len() == 1 {
                            // Another request got the same challenge and stored the
                            // nonce before this response arrived.
                            factory.record_nonce(uri, "rs-nonce");
                            return Err(StreamableHttpError::<reqwest::Error>::AuthRequired(
                                AuthRequiredError::new(r#"DPoP error="use_dpop_nonce""#.into())
                                    .with_dpop_nonce("rs-nonce"),
                            ));
                        }
                        Ok(())
                    }
                },
            )
            .await;

        result.unwrap();
        let calls = recorded.lock().unwrap();
        assert_eq!(calls.len(), 2);
        let first = proof_claims(&header(&calls[0].1, "dpop").unwrap());
        let second = proof_claims(&header(&calls[1].1, "dpop").unwrap());
        assert!(first.get("nonce").is_none());
        assert_eq!(second["nonce"], "rs-nonce");
    }
}
