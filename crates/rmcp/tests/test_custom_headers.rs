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
