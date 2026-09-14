use std::collections::HashMap;

use http::{HeaderName, HeaderValue};
use tracing::debug;

use crate::transport::{
    auth::{AuthClient, AuthError},
    streamable_http_client::{StreamableHttpClient, StreamableHttpError},
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
        auth_token: Option<String>,
        call: F,
    ) -> Result<T, StreamableHttpError<C::Error>>
    where
        F: Fn(Option<String>) -> Fut,
        Fut: Future<Output = Result<T, StreamableHttpError<C::Error>>>,
    {
        // Missing credentials are not an error in the reactive model: the
        // request goes out unauthenticated and the server's 401 challenge
        // drives authorization.
        let auth_token = match auth_token {
            None => match self.get_access_token().await {
                Ok(token) => Some(token),
                Err(AuthError::AuthorizationRequired) => None,
                Err(error) => return Err(error.into()),
            },
            token => token,
        };
        match call(auth_token.clone()).await {
            Err(StreamableHttpError::AuthRequired(challenge)) => {
                // One silent recovery attempt: refresh the rejected token and
                // retry only when the refresh actually produced a new one.
                let Some(sent_token) = auth_token else {
                    return Err(StreamableHttpError::AuthRequired(challenge));
                };
                let refreshed = {
                    let manager = self.auth_manager.lock().await;
                    manager.try_refresh_or_reauth().await
                };
                match refreshed {
                    Ok(fresh_token) if fresh_token != sent_token => call(Some(fresh_token)).await,
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
        self.call_reacting_to_challenges(auth_token, |token| {
            let uri = uri.clone();
            let session_id = session_id.clone();
            let custom_headers = custom_headers.clone();
            async move {
                self.http_client
                    .delete_session(uri, session_id, token, custom_headers)
                    .await
            }
        })
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
        self.call_reacting_to_challenges(auth_token, |token| {
            let uri = uri.clone();
            let session_id = session_id.clone();
            let last_event_id = last_event_id.clone();
            let custom_headers = custom_headers.clone();
            async move {
                self.http_client
                    .get_stream(uri, session_id, last_event_id, token, custom_headers)
                    .await
            }
        })
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
        self.call_reacting_to_challenges(auth_token, |token| {
            let uri = uri.clone();
            let session_id = session_id.clone();
            let last_event_id = last_event_id.clone();
            let custom_headers = custom_headers.clone();
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
        })
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
        self.call_reacting_to_challenges(auth_token, |token| {
            let uri = uri.clone();
            let message = message.clone();
            let session_id = session_id.clone();
            let custom_headers = custom_headers.clone();
            async move {
                self.http_client
                    .post_message(uri, message, session_id, token, custom_headers)
                    .await
            }
        })
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
        self.call_reacting_to_challenges(auth_token, |token| {
            let uri = uri.clone();
            let message = message.clone();
            let session_id = session_id.clone();
            let custom_headers = custom_headers.clone();
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
        })
        .await
    }
}

#[cfg(all(test, feature = "transport-streamable-http-client-reqwest"))]
mod tests {
    use std::sync::Arc;

    use oauth2::{AccessToken, RefreshToken, basic::BasicTokenType};

    use super::*;
    use crate::transport::{
        auth::{
            AuthorizationManager, AuthorizationMetadata, CredentialRefreshGuard, CredentialStore,
            InMemoryCredentialStore, OAuthHttpClient, OAuthHttpClientFuture, OAuthHttpRequest,
            OAuthTokenResponse, StoredCredentials, VendorExtraTokenFields,
        },
        streamable_http_client::AuthRequiredError,
    };

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
            .call_reacting_to_challenges(Some("old-token".into()), |_| async {
                Err::<(), _>(StreamableHttpError::AuthRequired(AuthRequiredError::new(
                    "Bearer".into(),
                )))
            })
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
            .call_reacting_to_challenges(Some("old-token".into()), |_| async {
                Err::<(), _>(StreamableHttpError::AuthRequired(AuthRequiredError::new(
                    "Bearer".into(),
                )))
            })
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
