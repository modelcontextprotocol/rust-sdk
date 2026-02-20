use std::collections::HashMap;

use http::{HeaderName, HeaderValue};

#[test]
fn test_config_custom_headers_default_empty() {
    use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;

    let config = StreamableHttpClientTransportConfig::with_uri("http://localhost:8080");
    assert!(
        config.custom_headers.is_empty(),
        "Default custom_headers should be empty"
    );
}

#[test]
fn test_config_custom_headers_builder() {
    use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;

    let mut headers = HashMap::new();
    headers.insert(
        HeaderName::from_static("x-test-header"),
        HeaderValue::from_static("test-value"),
    );

    let config = StreamableHttpClientTransportConfig::with_uri("http://localhost:8080")
        .custom_headers(headers);

    assert_eq!(config.custom_headers.len(), 1);
    assert_eq!(
        config
            .custom_headers
            .get(&HeaderName::from_static("x-test-header")),
        Some(&HeaderValue::from_static("test-value"))
    );
}

#[test]
fn test_config_custom_headers_multiple_values() {
    use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;

    let mut headers = HashMap::new();
    headers.insert(
        HeaderName::from_static("x-header-1"),
        HeaderValue::from_static("value-1"),
    );
    headers.insert(
        HeaderName::from_static("x-header-2"),
        HeaderValue::from_static("value-2"),
    );
    headers.insert(
        HeaderName::from_static("authorization"),
        HeaderValue::from_static("Bearer token123"),
    );

    let config = StreamableHttpClientTransportConfig::with_uri("http://localhost:8080")
        .custom_headers(headers);

    assert_eq!(config.custom_headers.len(), 3);
    assert_eq!(
        config
            .custom_headers
            .get(&HeaderName::from_static("x-header-1")),
        Some(&HeaderValue::from_static("value-1"))
    );
    assert_eq!(
        config
            .custom_headers
            .get(&HeaderName::from_static("x-header-2")),
        Some(&HeaderValue::from_static("value-2"))
    );
    assert_eq!(
        config
            .custom_headers
            .get(&HeaderName::from_static("authorization")),
        Some(&HeaderValue::from_static("Bearer token123"))
    );
}

#[test]
fn test_config_auth_header_and_custom_headers_together() {
    use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;

    let mut headers = HashMap::new();
    headers.insert(
        HeaderName::from_static("x-custom-header"),
        HeaderValue::from_static("custom-value"),
    );

    let config = StreamableHttpClientTransportConfig::with_uri("http://localhost:8080")
        .auth_header("my-bearer-token")
        .custom_headers(headers);

    assert_eq!(config.auth_header, Some("my-bearer-token".to_string()));
    assert_eq!(
        config
            .custom_headers
            .get(&HeaderName::from_static("x-custom-header")),
        Some(&HeaderValue::from_static("custom-value"))
    );
}

/// Unit test: post_message should reject reserved header "accept"
#[tokio::test]
#[cfg(feature = "transport-streamable-http-client-reqwest")]
async fn test_post_message_rejects_accept_header() {
    use std::sync::Arc;

    use rmcp::{
        model::{ClientJsonRpcMessage, ClientRequest, PingRequest, RequestId},
        transport::streamable_http_client::{StreamableHttpClient, StreamableHttpError},
    };

    let client = reqwest::Client::new();
    let mut custom_headers = HashMap::new();
    custom_headers.insert(
        HeaderName::from_static("accept"),
        HeaderValue::from_static("text/html"),
    );

    let message = ClientJsonRpcMessage::request(
        ClientRequest::PingRequest(PingRequest::default()),
        RequestId::Number(1),
    );

    let result = client
        .post_message(
            Arc::from("http://localhost:9999/mcp"),
            message,
            None,
            None,
            custom_headers,
        )
        .await;

    assert!(result.is_err(), "Should reject 'accept' header");
    match result {
        Err(StreamableHttpError::ReservedHeaderConflict(header_name)) => {
            assert_eq!(
                header_name, "accept",
                "Error should indicate 'accept' header"
            );
        }
        other => panic!("Expected ReservedHeaderConflict error, got: {:?}", other),
    }
}

/// Unit test: post_message should reject reserved header "mcp-session-id"
#[tokio::test]
#[cfg(feature = "transport-streamable-http-client-reqwest")]
async fn test_post_message_rejects_mcp_session_id() {
    use std::sync::Arc;

    use rmcp::{
        model::{ClientJsonRpcMessage, ClientRequest, PingRequest, RequestId},
        transport::streamable_http_client::{StreamableHttpClient, StreamableHttpError},
    };

    let client = reqwest::Client::new();
    let mut custom_headers = HashMap::new();
    custom_headers.insert(
        HeaderName::from_static("mcp-session-id"),
        HeaderValue::from_static("my-session"),
    );

    let message = ClientJsonRpcMessage::request(
        ClientRequest::PingRequest(PingRequest::default()),
        RequestId::Number(1),
    );

    let result = client
        .post_message(
            Arc::from("http://localhost:9999/mcp"),
            message,
            None,
            None,
            custom_headers,
        )
        .await;

    assert!(result.is_err(), "Should reject 'mcp-session-id' header");
    match result {
        Err(StreamableHttpError::ReservedHeaderConflict(header_name)) => {
            assert_eq!(
                header_name, "mcp-session-id",
                "Error should indicate 'mcp-session-id' header"
            );
        }
        other => panic!("Expected ReservedHeaderConflict error, got: {:?}", other),
    }
}

/// Unit test: post_message should allow the mcp-protocol-version header through
/// (it is injected by the worker after initialization, not a user-settable custom header)
#[tokio::test]
#[cfg(feature = "transport-streamable-http-client-reqwest")]
async fn test_post_message_allows_mcp_protocol_version() {
    use std::sync::Arc;

    use rmcp::{
        model::{ClientJsonRpcMessage, ClientRequest, PingRequest, RequestId},
        transport::streamable_http_client::StreamableHttpClient,
    };

    let client = reqwest::Client::new();
    let mut custom_headers = HashMap::new();
    custom_headers.insert(
        HeaderName::from_static("mcp-protocol-version"),
        HeaderValue::from_static("2025-03-26"),
    );

    let message = ClientJsonRpcMessage::request(
        ClientRequest::PingRequest(PingRequest::default()),
        RequestId::Number(1),
    );

    let result = client
        .post_message(
            Arc::from("http://localhost:9999/mcp"),
            message,
            None,
            None,
            custom_headers,
        )
        .await;

    // The header should be allowed through (not rejected as reserved).
    // The error should be a connection error (no server at localhost:9999),
    // not a ReservedHeaderConflict.
    assert!(result.is_err(), "Should fail due to connection error");
    assert!(
        !matches!(
            &result,
            Err(rmcp::transport::streamable_http_client::StreamableHttpError::ReservedHeaderConflict(
                _
            ))
        ),
        "MCP-Protocol-Version should not be rejected as reserved, got: {:?}",
        result
    );
}

/// Unit test: post_message should reject reserved header "last-event-id"
#[tokio::test]
#[cfg(feature = "transport-streamable-http-client-reqwest")]
async fn test_post_message_rejects_last_event_id() {
    use std::sync::Arc;

    use rmcp::{
        model::{ClientJsonRpcMessage, ClientRequest, PingRequest, RequestId},
        transport::streamable_http_client::{StreamableHttpClient, StreamableHttpError},
    };

    let client = reqwest::Client::new();
    let mut custom_headers = HashMap::new();
    custom_headers.insert(
        HeaderName::from_static("last-event-id"),
        HeaderValue::from_static("event-123"),
    );

    let message = ClientJsonRpcMessage::request(
        ClientRequest::PingRequest(PingRequest::default()),
        RequestId::Number(1),
    );

    let result = client
        .post_message(
            Arc::from("http://localhost:9999/mcp"),
            message,
            None,
            None,
            custom_headers,
        )
        .await;

    assert!(result.is_err(), "Should reject 'last-event-id' header");
    match result {
        Err(StreamableHttpError::ReservedHeaderConflict(header_name)) => {
            assert_eq!(
                header_name, "last-event-id",
                "Error should indicate 'last-event-id' header"
            );
        }
        other => panic!("Expected ReservedHeaderConflict error, got: {:?}", other),
    }
}

/// Unit test: post_message should do case-insensitive matching for reserved headers
#[tokio::test]
#[cfg(feature = "transport-streamable-http-client-reqwest")]
async fn test_post_message_case_insensitive_matching() {
    use std::sync::Arc;

    use rmcp::{
        model::{ClientJsonRpcMessage, ClientRequest, PingRequest, RequestId},
        transport::streamable_http_client::{StreamableHttpClient, StreamableHttpError},
    };

    let client = reqwest::Client::new();
    let message = ClientJsonRpcMessage::request(
        ClientRequest::PingRequest(PingRequest::default()),
        RequestId::Number(1),
    );

    // Test different casings
    let test_cases = vec![
        ("Accept", "Should reject 'Accept' (capitalized)"),
        ("ACCEPT", "Should reject 'ACCEPT' (uppercase)"),
        ("Mcp-Session-Id", "Should reject 'Mcp-Session-Id'"),
        ("MCP-SESSION-ID", "Should reject 'MCP-SESSION-ID'"),
    ];

    for (header_name, error_msg) in test_cases {
        let mut custom_headers = HashMap::new();
        custom_headers.insert(
            HeaderName::from_bytes(header_name.as_bytes()).unwrap(),
            HeaderValue::from_static("value"),
        );

        let result = client
            .post_message(
                Arc::from("http://localhost:9999/mcp"),
                message.clone(),
                None,
                None,
                custom_headers,
            )
            .await;

        assert!(result.is_err(), "{}", error_msg);
        if let Err(StreamableHttpError::ReservedHeaderConflict(_)) = result {
            // Success
        } else {
            panic!(
                "{}: Expected ReservedHeaderConflict, got: {:?}",
                error_msg, result
            );
        }
    }
}

/// Integration test: Verify that custom headers are actually sent in MCP HTTP requests
#[tokio::test]
#[cfg(all(
    feature = "transport-streamable-http-client",
    feature = "transport-streamable-http-client-reqwest"
))]
async fn test_mcp_custom_headers_sent_to_server() -> anyhow::Result<()> {
    use std::{net::SocketAddr, sync::Arc};

    use axum::{
        Router, body::Bytes, extract::State, http::StatusCode, response::IntoResponse,
        routing::post,
    };
    use rmcp::{
        ServiceExt,
        transport::{
            StreamableHttpClientTransport,
            streamable_http_client::StreamableHttpClientTransportConfig,
        },
    };
    use serde_json::json;
    use tokio::sync::Mutex;

    // State to capture received headers
    #[derive(Clone)]
    struct ServerState {
        received_headers: Arc<Mutex<HashMap<String, String>>>,
        initialize_called: Arc<tokio::sync::Notify>,
    }

    // Handler that captures headers from MCP requests
    async fn mcp_handler(
        State(state): State<ServerState>,
        headers: http::HeaderMap,
        body: Bytes,
    ) -> impl IntoResponse {
        // Capture all custom headers (starting with x-)
        let mut headers_map = HashMap::new();
        for (name, value) in headers.iter() {
            let name_str = name.as_str();
            if name_str.starts_with("x-") {
                if let Ok(v) = value.to_str() {
                    headers_map.insert(name_str.to_string(), v.to_string());
                }
            }
        }

        // Store captured headers
        let mut stored = state.received_headers.lock().await;
        stored.extend(headers_map);

        // Parse the MCP request
        if let Ok(json_body) = serde_json::from_slice::<serde_json::Value>(&body) {
            if let Some(method) = json_body.get("method").and_then(|m| m.as_str()) {
                if method == "initialize" {
                    state.initialize_called.notify_one();
                    // Return a valid MCP initialize response with session header
                    let response = json!({
                        "jsonrpc": "2.0",
                        "id": json_body.get("id"),
                        "result": {
                            "protocolVersion": "2024-11-05",
                            "capabilities": {},
                            "serverInfo": {
                                "name": "test-server",
                                "version": "1.0.0"
                            }
                        }
                    });
                    return (
                        StatusCode::OK,
                        [
                            (http::header::CONTENT_TYPE, "application/json"),
                            (
                                http::HeaderName::from_static("mcp-session-id"),
                                "test-session-123",
                            ),
                        ],
                        response.to_string(),
                    );
                } else if method == "notifications/initialized" {
                    // For initialized notification, return 202 Accepted
                    return (
                        StatusCode::ACCEPTED,
                        [
                            (http::header::CONTENT_TYPE, "application/json"),
                            (
                                http::HeaderName::from_static("mcp-session-id"),
                                "test-session-123",
                            ),
                        ],
                        String::new(),
                    );
                }
            }
        }

        // Default response for other requests
        let response = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {}
        });
        (
            StatusCode::OK,
            [
                (http::header::CONTENT_TYPE, "application/json"),
                (
                    http::HeaderName::from_static("mcp-session-id"),
                    "test-session-123",
                ),
            ],
            response.to_string(),
        )
    }

    // Setup test server
    let state = ServerState {
        received_headers: Arc::new(Mutex::new(HashMap::new())),
        initialize_called: Arc::new(tokio::sync::Notify::new()),
    };

    let app = Router::new()
        .route("/mcp", post(mcp_handler))
        .with_state(state.clone());

    let addr = SocketAddr::from(([127, 0, 0, 1], 0));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let port = listener.local_addr()?.port();

    let server_handle = tokio::spawn(async move { axum::serve(listener, app).await });

    // Wait for server to be ready
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Create MCP client with custom headers
    let mut custom_headers = HashMap::new();
    custom_headers.insert(
        HeaderName::from_static("x-test-header"),
        HeaderValue::from_static("test-value-123"),
    );
    custom_headers.insert(
        HeaderName::from_static("x-another-header"),
        HeaderValue::from_static("another-value-456"),
    );
    custom_headers.insert(
        HeaderName::from_static("x-client-id"),
        HeaderValue::from_static("test-client"),
    );

    let config =
        StreamableHttpClientTransportConfig::with_uri(format!("http://127.0.0.1:{}/mcp", port))
            .custom_headers(custom_headers);

    let transport = StreamableHttpClientTransport::from_config(config);

    // Start MCP client with empty handler (this will trigger initialize request)
    let client = ().serve(transport).await.expect("Failed to start client");

    // Wait for initialize to be called
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        state.initialize_called.notified(),
    )
    .await
    .expect("Initialize request should be received");

    // Verify that custom headers were received
    let headers = state.received_headers.lock().await;

    assert_eq!(
        headers.get("x-test-header"),
        Some(&"test-value-123".to_string()),
        "Custom header x-test-header should be sent to MCP server"
    );
    assert_eq!(
        headers.get("x-another-header"),
        Some(&"another-value-456".to_string()),
        "Custom header x-another-header should be sent to MCP server"
    );
    assert_eq!(
        headers.get("x-client-id"),
        Some(&"test-client".to_string()),
        "Custom header x-client-id should be sent to MCP server"
    );

    // Cleanup
    drop(client);
    server_handle.abort();

    Ok(())
}

/// Integration test: Verify that MCP-Protocol-Version header is sent on post-init requests
#[tokio::test]
#[cfg(all(
    feature = "transport-streamable-http-client",
    feature = "transport-streamable-http-client-reqwest"
))]
async fn test_mcp_protocol_version_header_sent_after_init() -> anyhow::Result<()> {
    use std::{net::SocketAddr, sync::Arc};

    use axum::{
        Router, body::Bytes, extract::State, http::StatusCode, response::IntoResponse,
        routing::post,
    };
    use rmcp::{
        ServiceExt,
        transport::{
            StreamableHttpClientTransport,
            streamable_http_client::StreamableHttpClientTransportConfig,
        },
    };
    use serde_json::json;
    use tokio::sync::Mutex;

    type CapturedRequests = Vec<(String, Option<String>)>;

    #[derive(Clone)]
    struct ServerState {
        /// Captures the MCP-Protocol-Version header value for each request method
        protocol_version_by_method: Arc<Mutex<CapturedRequests>>,
        initialized_called: Arc<tokio::sync::Notify>,
    }

    async fn mcp_handler(
        State(state): State<ServerState>,
        headers: http::HeaderMap,
        body: Bytes,
    ) -> impl IntoResponse {
        let protocol_version = headers
            .get("mcp-protocol-version")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        if let Ok(json_body) = serde_json::from_slice::<serde_json::Value>(&body) {
            let method = json_body
                .get("method")
                .and_then(|m| m.as_str())
                .unwrap_or("unknown")
                .to_string();

            state
                .protocol_version_by_method
                .lock()
                .await
                .push((method.clone(), protocol_version));

            if method == "initialize" {
                let response = json!({
                    "jsonrpc": "2.0",
                    "id": json_body.get("id"),
                    "result": {
                        "protocolVersion": "2025-03-26",
                        "capabilities": {},
                        "serverInfo": {
                            "name": "test-server",
                            "version": "1.0.0"
                        }
                    }
                });
                return (
                    StatusCode::OK,
                    [
                        (http::header::CONTENT_TYPE, "application/json"),
                        (
                            http::HeaderName::from_static("mcp-session-id"),
                            "test-session-456",
                        ),
                    ],
                    response.to_string(),
                );
            } else if method == "notifications/initialized" {
                state.initialized_called.notify_one();
                return (
                    StatusCode::ACCEPTED,
                    [
                        (http::header::CONTENT_TYPE, "application/json"),
                        (
                            http::HeaderName::from_static("mcp-session-id"),
                            "test-session-456",
                        ),
                    ],
                    String::new(),
                );
            }
        }

        let response = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {}
        });
        (
            StatusCode::OK,
            [
                (http::header::CONTENT_TYPE, "application/json"),
                (
                    http::HeaderName::from_static("mcp-session-id"),
                    "test-session-456",
                ),
            ],
            response.to_string(),
        )
    }

    let state = ServerState {
        protocol_version_by_method: Arc::new(Mutex::new(Vec::new())),
        initialized_called: Arc::new(tokio::sync::Notify::new()),
    };

    let app = Router::new()
        .route("/mcp", post(mcp_handler))
        .with_state(state.clone());

    let addr = SocketAddr::from(([127, 0, 0, 1], 0));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let port = listener.local_addr()?.port();

    let server_handle = tokio::spawn(async move { axum::serve(listener, app).await });
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let config =
        StreamableHttpClientTransportConfig::with_uri(format!("http://127.0.0.1:{}/mcp", port));

    let transport = StreamableHttpClientTransport::from_config(config);
    let client = ().serve(transport).await.expect("Failed to start client");

    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        state.initialized_called.notified(),
    )
    .await
    .expect("Initialized notification should be received");

    // Give time for the initialized notification to be fully processed
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let captured = state.protocol_version_by_method.lock().await;

    // The initialize request should NOT have MCP-Protocol-Version
    // (the version isn't known yet)
    let init_entry = captured
        .iter()
        .find(|(m, _)| m == "initialize")
        .expect("Should have captured initialize request");
    assert_eq!(
        init_entry.1, None,
        "Initialize request should not have MCP-Protocol-Version header"
    );

    // The initialized notification should HAVE MCP-Protocol-Version
    let initialized_entry = captured
        .iter()
        .find(|(m, _)| m == "notifications/initialized")
        .expect("Should have captured initialized notification");
    assert_eq!(
        initialized_entry.1,
        Some("2025-03-26".to_string()),
        "Initialized notification should include MCP-Protocol-Version: 2025-03-26"
    );

    drop(client);
    server_handle.abort();

    Ok(())
}

/// Integration test: Verify server rejects unsupported MCP-Protocol-Version with 400
#[tokio::test]
#[cfg(all(feature = "transport-streamable-http-server", feature = "server",))]
async fn test_server_rejects_unsupported_protocol_version() {
    use std::sync::Arc;

    use bytes::Bytes;
    use http::{Method, Request, header::CONTENT_TYPE};
    use http_body_util::Full;
    use rmcp::{
        handler::server::ServerHandler,
        model::{ServerCapabilities, ServerInfo},
        transport::streamable_http_server::{
            StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
        },
    };
    use serde_json::json;

    #[derive(Clone)]
    struct TestHandler;

    impl ServerHandler for TestHandler {
        fn get_info(&self) -> ServerInfo {
            ServerInfo {
                capabilities: ServerCapabilities::builder().build(),
                ..Default::default()
            }
        }
    }

    let session_manager = Arc::new(LocalSessionManager::default());
    let service = StreamableHttpService::new(
        || Ok(TestHandler),
        session_manager,
        StreamableHttpServerConfig::default(),
    );

    // First, send an initialize request to create a session
    let init_body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-03-26",
            "capabilities": {},
            "clientInfo": {
                "name": "test-client",
                "version": "1.0.0"
            }
        }
    });

    let init_request = Request::builder()
        .method(Method::POST)
        .header("Accept", "application/json, text/event-stream")
        .header(CONTENT_TYPE, "application/json")
        .body(Full::new(Bytes::from(init_body.to_string())))
        .unwrap();

    let response = service.handle(init_request).await;
    assert_eq!(response.status(), http::StatusCode::OK);

    // Extract session id from response
    let session_id = response
        .headers()
        .get("mcp-session-id")
        .expect("Should have session id")
        .to_str()
        .unwrap()
        .to_string();

    // Send initialized notification to complete handshake
    let initialized_body = json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    });
    let initialized_request = Request::builder()
        .method(Method::POST)
        .header("Accept", "application/json, text/event-stream")
        .header(CONTENT_TYPE, "application/json")
        .header("mcp-session-id", &session_id)
        .header("mcp-protocol-version", "2025-03-26")
        .body(Full::new(Bytes::from(initialized_body.to_string())))
        .unwrap();

    let response = service.handle(initialized_request).await;
    assert_eq!(response.status(), http::StatusCode::ACCEPTED);

    // Test 1: Valid protocol version should succeed
    let valid_body = json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    });
    let valid_request = Request::builder()
        .method(Method::POST)
        .header("Accept", "application/json, text/event-stream")
        .header(CONTENT_TYPE, "application/json")
        .header("mcp-session-id", &session_id)
        .header("mcp-protocol-version", "2025-03-26")
        .body(Full::new(Bytes::from(valid_body.to_string())))
        .unwrap();

    let response = service.handle(valid_request).await;
    assert_eq!(
        response.status(),
        http::StatusCode::ACCEPTED,
        "Valid MCP-Protocol-Version should be accepted"
    );

    // Test 2: Unsupported protocol version should return 400
    let invalid_body = json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    });
    let invalid_request = Request::builder()
        .method(Method::POST)
        .header("Accept", "application/json, text/event-stream")
        .header(CONTENT_TYPE, "application/json")
        .header("mcp-session-id", &session_id)
        .header("mcp-protocol-version", "9999-01-01")
        .body(Full::new(Bytes::from(invalid_body.to_string())))
        .unwrap();

    let response = service.handle(invalid_request).await;
    assert_eq!(
        response.status(),
        http::StatusCode::BAD_REQUEST,
        "Unsupported MCP-Protocol-Version should return 400"
    );

    // Test 3: Missing protocol version should succeed (backwards compat)
    let no_version_body = json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    });
    let no_version_request = Request::builder()
        .method(Method::POST)
        .header("Accept", "application/json, text/event-stream")
        .header(CONTENT_TYPE, "application/json")
        .header("mcp-session-id", &session_id)
        .body(Full::new(Bytes::from(no_version_body.to_string())))
        .unwrap();

    let response = service.handle(no_version_request).await;
    assert_eq!(
        response.status(),
        http::StatusCode::ACCEPTED,
        "Missing MCP-Protocol-Version should be accepted (backwards compat)"
    );
}

/// Unit test: ProtocolVersion::as_str and KNOWN_VERSIONS
#[test]
fn test_protocol_version_utilities() {
    use rmcp::model::ProtocolVersion;

    assert_eq!(ProtocolVersion::V_2025_06_18.as_str(), "2025-06-18");
    assert_eq!(ProtocolVersion::V_2025_03_26.as_str(), "2025-03-26");
    assert_eq!(ProtocolVersion::V_2024_11_05.as_str(), "2024-11-05");

    assert_eq!(ProtocolVersion::KNOWN_VERSIONS.len(), 3);
    assert!(ProtocolVersion::KNOWN_VERSIONS.contains(&ProtocolVersion::V_2024_11_05));
    assert!(ProtocolVersion::KNOWN_VERSIONS.contains(&ProtocolVersion::V_2025_03_26));
    assert!(ProtocolVersion::KNOWN_VERSIONS.contains(&ProtocolVersion::V_2025_06_18));
}
