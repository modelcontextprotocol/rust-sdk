use std::collections::HashMap;

use http::{HeaderName, HeaderValue};
use tracing::debug;

use crate::transport::{
    auth::{AuthClient, AuthError},
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
    ///   retry, then the challenge propagates.
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
                    Err(error @ AuthError::CredentialStoreError(_)) => Err(error.into()),
                    Err(error) => {
                        debug!("token refresh after server rejection failed: {error}");
                        Err(StreamableHttpError::AuthRequired(challenge))
                    }
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
        self.call_reacting_to_challenges(auth_token, |token| {
            let uri = uri.clone();
            let message = message.clone();
            let session_id = session_id.clone();
            let custom_headers = custom_headers.clone();
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
        })
        .await
    }
}

#[cfg(all(test, feature = "transport-streamable-http-client-reqwest"))]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::transport::{
        auth::{
            AuthorizationManager, AuthorizationMetadata, CredentialRefreshGuard, CredentialStore,
            InMemoryCredentialStore, OAuthHttpClient, OAuthHttpClientFuture, OAuthHttpRequest,
            StoredCredentials,
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
                    limit: limits.max_json_response_size,
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
            Err(StreamableHttpError::ResponseBodyTooLarge { limit: 31 })
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
}
