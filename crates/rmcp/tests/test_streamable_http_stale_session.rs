#![cfg(all(
    feature = "transport-streamable-http-client",
    feature = "transport-streamable-http-client-reqwest",
    feature = "transport-streamable-http-server",
    not(feature = "local")
))]

use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
};

use futures::stream;
use http::{HeaderName, HeaderValue};
use rmcp::{
    ServiceError, ServiceExt,
    model::{
        CallToolRequestParams, ClientInfo, ClientJsonRpcMessage, ClientRequest, ErrorCode,
        ErrorData, InitializeResult, PingRequest, RequestId, ServerCapabilities,
        ServerJsonRpcMessage, ServerResult,
    },
    transport::{
        StreamableHttpClientTransport,
        streamable_http_client::{
            StreamableHttpClient, StreamableHttpClientTransportConfig, StreamableHttpError,
            StreamableHttpPostResponse,
        },
        streamable_http_server::{
            StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
        },
    },
};
use tokio_util::sync::CancellationToken;

mod common;
use common::calculator::Calculator;

#[derive(Debug, thiserror::Error)]
#[error("mock streamable http client error")]
struct MockClientError;

#[derive(Clone)]
struct ReinitDropsAcceptedResponseClient {
    state: Arc<tokio::sync::Mutex<MockState>>,
    stale_stream_cancelled: CancellationToken,
    initial_request_accepted: Arc<tokio::sync::Semaphore>,
    final_retry_accepted: Arc<tokio::sync::Semaphore>,
}

struct MockState {
    session_counter: usize,
    posts: VecDeque<MockPost>,
}

enum MockPost {
    Initialize,
    Initialized,
    Accepted,
    SessionExpired,
}

impl ReinitDropsAcceptedResponseClient {
    fn new() -> Self {
        Self {
            state: Arc::new(tokio::sync::Mutex::new(MockState {
                session_counter: 0,
                posts: VecDeque::from([
                    MockPost::Initialize,
                    MockPost::Initialized,
                    MockPost::Accepted,
                    MockPost::SessionExpired,
                    MockPost::Initialize,
                    MockPost::Initialized,
                    MockPost::Accepted,
                ]),
            })),
            stale_stream_cancelled: CancellationToken::new(),
            initial_request_accepted: Arc::new(tokio::sync::Semaphore::new(0)),
            final_retry_accepted: Arc::new(tokio::sync::Semaphore::new(0)),
        }
    }
}

impl StreamableHttpClient for ReinitDropsAcceptedResponseClient {
    type Error = MockClientError;

    async fn post_message(
        &self,
        _uri: Arc<str>,
        message: ClientJsonRpcMessage,
        _session_id: Option<Arc<str>>,
        _auth_header: Option<String>,
        _custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>> {
        let mut state = self.state.lock().await;
        match state
            .posts
            .pop_front()
            .expect("unexpected mock post_message call")
        {
            MockPost::Initialize => {
                state.session_counter += 1;
                let id = match message {
                    ClientJsonRpcMessage::Request(request) => request.id,
                    other => panic!("expected initialize request, got {other:?}"),
                };
                Ok(StreamableHttpPostResponse::Json(
                    ServerJsonRpcMessage::response(
                        ServerResult::InitializeResult(InitializeResult::new(
                            ServerCapabilities::builder().enable_tools().build(),
                        )),
                        id,
                    ),
                    Some(format!("session-{}", state.session_counter)),
                ))
            }
            MockPost::Initialized => {
                assert!(
                    matches!(message, ClientJsonRpcMessage::Notification(_)),
                    "expected initialized notification, got {message:?}"
                );
                Ok(StreamableHttpPostResponse::Accepted)
            }
            MockPost::Accepted => {
                if state.posts.is_empty() {
                    self.final_retry_accepted.add_permits(1);
                } else {
                    self.initial_request_accepted.add_permits(1);
                }
                Ok(StreamableHttpPostResponse::Accepted)
            }
            MockPost::SessionExpired => Err(StreamableHttpError::SessionExpired),
        }
    }

    async fn delete_session(
        &self,
        _uri: Arc<str>,
        _session_id: Arc<str>,
        _auth_header: Option<String>,
        _custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<(), StreamableHttpError<Self::Error>> {
        Ok(())
    }

    async fn get_stream(
        &self,
        _uri: Arc<str>,
        session_id: Arc<str>,
        _last_event_id: Option<String>,
        _auth_header: Option<String>,
        _custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<
        futures::stream::BoxStream<'static, Result<sse_stream::Sse, sse_stream::Error>>,
        StreamableHttpError<Self::Error>,
    > {
        if session_id.as_ref() == "session-1" {
            let cancel = self.stale_stream_cancelled.clone();
            Ok(Box::pin(stream::once(async move {
                cancel.cancelled_owned().await;
                Ok(sse_stream::Sse {
                    event: None,
                    data: Some(
                        serde_json::to_string(&ServerJsonRpcMessage::error(
                            ErrorData::new(
                                ErrorCode::INTERNAL_ERROR,
                                "stale stream should not deliver after re-init",
                                None,
                            ),
                            Some(RequestId::Number(2)),
                        ))
                        .expect("serialize stale error"),
                    ),
                    id: None,
                    retry: None,
                })
            })))
        } else {
            Ok(Box::pin(stream::pending()))
        }
    }
}

#[tokio::test]
async fn test_reinitialization_completes_accepted_sse_request_instead_of_hanging()
-> anyhow::Result<()> {
    let mock_client = ReinitDropsAcceptedResponseClient::new();
    let initial_request_accepted = mock_client.initial_request_accepted.clone();
    let final_retry_accepted = mock_client.final_retry_accepted.clone();
    let transport = StreamableHttpClientTransport::with_client(
        mock_client,
        StreamableHttpClientTransportConfig::with_uri("mock://mcp"),
    );
    let mut client = ClientInfo::default().serve(transport).await?;

    let peer = client.peer().clone();
    let pending_call = tokio::spawn(async move {
        peer.call_tool(CallToolRequestParams::new("slow_tool"))
            .await
    });

    let _initial_permit = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        initial_request_accepted.acquire(),
    )
    .await
    .expect("initial accepted request should be observed")
    .expect("initial accepted request semaphore should stay open");

    let reinit_trigger = {
        let peer = client.peer().clone();
        tokio::spawn(async move { peer.list_tools(None).await })
    };

    let _retry_permit = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        final_retry_accepted.acquire(),
    )
    .await
    .expect("re-initialization retry should be accepted")
    .expect("re-initialization retry semaphore should stay open");

    let err = tokio::time::timeout(std::time::Duration::from_millis(100), pending_call)
        .await
        .expect("accepted SSE-backed request should complete instead of hanging")?
        .expect_err(
            "accepted request should fail after re-initialization drops its response stream",
        );

    match err {
        ServiceError::McpError(error) => {
            assert_eq!(error.code, ErrorCode::INTERNAL_ERROR);
            assert!(
                error.message.contains("session"),
                "expected session-related error, got: {error}"
            );
        }
        other => panic!("expected McpError for orphaned request, got: {other:?}"),
    }

    reinit_trigger.abort();
    let _ = client.close().await;

    Ok(())
}

#[tokio::test]
async fn test_stale_session_id_returns_status_aware_error() -> anyhow::Result<()> {
    let ct = CancellationToken::new();
    let service: StreamableHttpService<Calculator, LocalSessionManager> =
        StreamableHttpService::new(
            || Ok(Calculator::new()),
            Default::default(),
            StreamableHttpServerConfig::default()
                .with_sse_keep_alive(None)
                .with_cancellation_token(ct.child_token()),
        );

    let router = axum::Router::new().nest_service("/mcp", service);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;

    let handle = tokio::spawn({
        let ct = ct.clone();
        async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async move { ct.cancelled_owned().await })
                .await;
        }
    });

    let uri = Arc::<str>::from(format!("http://{addr}/mcp"));
    let message = ClientJsonRpcMessage::request(
        ClientRequest::PingRequest(PingRequest::default()),
        RequestId::Number(1),
    );

    let client = reqwest::Client::new();
    let result = client
        .post_message(
            uri.clone(),
            message,
            Some(Arc::from("stale-session-id")),
            None,
            HashMap::new(),
        )
        .await;

    let raw_response = reqwest::Client::new()
        .post(uri.as_ref())
        .header("accept", "application/json, text/event-stream")
        .header("content-type", "application/json")
        .header("mcp-session-id", "stale-session-id")
        .body(r#"{"jsonrpc":"2.0","id":1,"method":"ping","params":{}}"#)
        .send()
        .await?;

    assert_eq!(raw_response.status(), reqwest::StatusCode::NOT_FOUND);
    match result {
        Err(StreamableHttpError::SessionExpired) => {
            // Expected: post_message detects 404 with a session ID and returns SessionExpired
        }
        other => panic!("expected SessionExpired, got: {other:?}"),
    }

    ct.cancel();
    handle.await?;

    Ok(())
}

/// Verify that when the server loses a session (returns HTTP 404), the client
/// transparently re-initializes and the original request succeeds.
#[tokio::test]
async fn test_transparent_reinitialization_on_session_expiry() -> anyhow::Result<()> {
    let ct = CancellationToken::new();
    let session_manager = Arc::new(LocalSessionManager::default());

    let service = StreamableHttpService::new(
        || Ok(Calculator::new()),
        session_manager.clone(),
        StreamableHttpServerConfig::default()
            .with_sse_keep_alive(None)
            .with_cancellation_token(ct.child_token()),
    );

    let router = axum::Router::new().nest_service("/mcp", service);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;

    let server_handle = tokio::spawn({
        let ct = ct.clone();
        async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async move { ct.cancelled_owned().await })
                .await;
        }
    });

    // Connect a full client transport (this performs initialize + notifications/initialized)
    let transport = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(format!("http://{addr}/mcp"))
            .reinit_on_expired_session(true),
    );
    let client = ().serve(transport).await?;

    // Verify the session is established: list_all_resources() succeeds
    let _resources = client.list_all_resources().await?;

    // Capture the current session ID from the server
    let original_session_id = {
        let sessions = session_manager.sessions.read().await;
        sessions
            .keys()
            .next()
            .cloned()
            .expect("session should exist")
    };

    // Force session expiry by removing all sessions from the server-side manager
    {
        let mut sessions = session_manager.sessions.write().await;
        sessions.clear();
    }

    // This call should trigger transparent re-initialization and still succeed
    let _resources_after = client.list_all_resources().await?;

    // Verify the server created a new session with a different ID
    {
        let sessions = session_manager.sessions.read().await;
        let new_session_id = sessions
            .keys()
            .next()
            .expect("new session should exist after re-initialization");
        assert_ne!(
            new_session_id, &original_session_id,
            "new session ID should differ from the original"
        );
    }

    let _ = client.cancel().await;
    ct.cancel();
    server_handle.await?;

    Ok(())
}

/// Verify that when `reinit_on_expired_session` is false and the server loses the session,
/// the client receives a `SessionExpired` transport error instead of retrying.
#[tokio::test]
async fn test_session_expired_error_when_reinit_disabled() -> anyhow::Result<()> {
    let ct = CancellationToken::new();
    let session_manager = Arc::new(LocalSessionManager::default());

    let service = StreamableHttpService::new(
        || Ok(Calculator::new()),
        session_manager.clone(),
        StreamableHttpServerConfig::default()
            .with_sse_keep_alive(None)
            .with_cancellation_token(ct.child_token()),
    );

    let router = axum::Router::new().nest_service("/mcp", service);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;

    let server_handle = tokio::spawn({
        let ct = ct.clone();
        async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async move { ct.cancelled_owned().await })
                .await;
        }
    });

    let transport = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(format!("http://{addr}/mcp"))
            .reinit_on_expired_session(false),
    );
    let client = ().serve(transport).await?;

    // Verify the session is established
    let _resources = client.list_all_resources().await?;

    // Force session expiry by removing all sessions from the server-side manager
    {
        let mut sessions = session_manager.sessions.write().await;
        sessions.clear();
    }

    // This call should fail with a SessionExpired transport error
    let result = client.list_all_resources().await;
    match result {
        Err(ServiceError::TransportSend(ref dyn_err)) => {
            let err_msg = format!("{dyn_err}");
            assert!(
                err_msg.contains("Session expired"),
                "expected 'Session expired' in error message, got: {err_msg}"
            );
        }
        other => panic!("expected TransportSend(SessionExpired), got: {other:?}"),
    }

    let _ = client.cancel().await;
    ct.cancel();
    server_handle.await?;

    Ok(())
}
