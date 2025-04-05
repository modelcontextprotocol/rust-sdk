// cargo test --features "server client" --package rmcp test_logging
use std::{
    future::Future,
    sync::{Arc, Mutex},
};

use rmcp::{
    ClientHandler, Error as McpError, Peer, RoleClient, RoleServer, ServerHandler, ServiceExt,
    model::{
        LoggingLevel, LoggingMessageNotificationParam, ServerCapabilities, ServerInfo,
        SetLevelRequestParam,
    },
    service::RequestContext,
};
use tokio::sync::Notify;

pub struct LoggingClient {
    receive_signal: Arc<Notify>,
    received_messages: Arc<Mutex<Vec<LoggingMessageNotificationParam>>>,
    peer: Option<Peer<RoleClient>>,
}

impl ClientHandler for LoggingClient {
    async fn on_logging_message(&self, params: LoggingMessageNotificationParam) {
        println!("Client: Received log message: {:?}", params);
        let mut messages = self.received_messages.lock().unwrap();
        messages.push(params);
        self.receive_signal.notify_one();
    }

    fn set_peer(&mut self, peer: Peer<RoleClient>) {
        self.peer.replace(peer);
    }

    fn get_peer(&self) -> Option<Peer<RoleClient>> {
        self.peer.clone()
    }
}

pub struct TestServer {}

impl TestServer {
    fn new() -> Self {
        Self {}
    }
}

impl ServerHandler for TestServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            capabilities: ServerCapabilities::builder().enable_logging().build(),
            ..Default::default()
        }
    }

    fn set_level(
        &self,
        request: SetLevelRequestParam,
        context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<(), McpError>> + Send + '_ {
        let peer = context.peer;
        async move {
            let (data, logger) = match request.level {
                LoggingLevel::Error => (
                    serde_json::json!({
                        "message": "Failed to process request",
                        "error_code": "E1001",
                        "error_details": "Connection timeout",
                        "timestamp": chrono::Utc::now().to_rfc3339(),
                    }),
                    Some("error_handler".to_string()),
                ),
                LoggingLevel::Debug => (
                    serde_json::json!({
                        "message": "Processing request",
                        "function": "handle_request",
                        "line": 42,
                        "context": {
                            "request_id": "req-123",
                            "user_id": "user-456"
                        },
                        "timestamp": chrono::Utc::now().to_rfc3339(),
                    }),
                    Some("debug_logger".to_string()),
                ),
                LoggingLevel::Info => (
                    serde_json::json!({
                        "message": "System status update",
                        "status": "healthy",
                        "metrics": {
                            "requests_per_second": 150,
                            "average_latency_ms": 45,
                            "error_rate": 0.01
                        },
                        "timestamp": chrono::Utc::now().to_rfc3339(),
                    }),
                    Some("monitoring".to_string()),
                ),
                _ => (
                    serde_json::json!({
                        "message": format!("Message at level {:?}", request.level),
                        "timestamp": chrono::Utc::now().to_rfc3339(),
                    }),
                    None,
                ),
            };

            if let Err(e) = peer
                .notify_logging_message(LoggingMessageNotificationParam {
                    level: request.level,
                    data,
                    logger,
                })
                .await
            {
                panic!("Failed to send notification: {}", e);
            }
            Ok(())
        }
    }
}

#[tokio::test]
async fn test_logging_spec_compliance() -> anyhow::Result<()> {
    let (server_transport, client_transport) = tokio::io::duplex(4096);
    let receive_signal = Arc::new(Notify::new());
    let received_messages = Arc::new(Mutex::new(Vec::new()));

    // Start server
    tokio::spawn(async move {
        let server = TestServer::new().serve(server_transport).await?;

        // Test server can send messages before level is set
        server
            .peer()
            .notify_logging_message(LoggingMessageNotificationParam {
                level: LoggingLevel::Info,
                data: serde_json::json!({
                    "message": "Server initiated message",
                    "timestamp": chrono::Utc::now().to_rfc3339(),
                }),
                logger: Some("test_server".to_string()),
            })
            .await?;

        server.waiting().await?;
        anyhow::Ok(())
    });

    let client = LoggingClient {
        receive_signal: receive_signal.clone(),
        received_messages: received_messages.clone(),
        peer: None,
    }
    .serve(client_transport)
    .await?;

    // Verify server-initiated message
    receive_signal.notified().await;
    {
        let mut messages = received_messages.lock().unwrap();
        assert_eq!(messages.len(), 1, "Should receive server-initiated message");
        messages.clear();
    }

    // Test level filtering and message format
    for level in [
        LoggingLevel::Emergency,
        LoggingLevel::Warning,
        LoggingLevel::Debug,
    ] {
        client
            .peer()
            .set_level(SetLevelRequestParam { level })
            .await?;
        receive_signal.notified().await;

        let mut messages = received_messages.lock().unwrap();
        let msg = messages.last().unwrap();

        // Verify required fields
        assert_eq!(msg.level, level);
        assert!(msg.data.is_object());

        // Verify data format
        let data = msg.data.as_object().unwrap();
        assert!(data.contains_key("message"));
        assert!(data.contains_key("timestamp"));

        // Verify timestamp
        let timestamp = data["timestamp"].as_str().unwrap();
        chrono::DateTime::parse_from_rfc3339(timestamp).expect("RFC3339 timestamp");

        messages.clear();
    }

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn test_logging_user_scenarios() -> anyhow::Result<()> {
    let (server_transport, client_transport) = tokio::io::duplex(4096);
    let receive_signal = Arc::new(Notify::new());
    let received_messages = Arc::new(Mutex::new(Vec::new()));

    // Start server
    tokio::spawn(async move {
        let server = TestServer::new().serve(server_transport).await?;
        server.waiting().await?;
        anyhow::Ok(())
    });

    let client = LoggingClient {
        receive_signal: receive_signal.clone(),
        received_messages: received_messages.clone(),
        peer: None,
    }
    .serve(client_transport)
    .await?;

    // Test 1: Error reporting scenario
    // User should see detailed error information
    client
        .peer()
        .set_level(SetLevelRequestParam {
            level: LoggingLevel::Error,
        })
        .await?;
    receive_signal.notified().await;
    {
        let messages = received_messages.lock().unwrap();
        let msg = &messages[0];
        let data = msg.data.as_object().unwrap();
        assert!(
            data.contains_key("error_code"),
            "Error should have an error code"
        );
        assert!(
            data.contains_key("error_details"),
            "Error should have details"
        );
        assert!(
            data.contains_key("timestamp"),
            "Should know when error occurred"
        );
    }

    // Test 2: Debug scenario
    // User debugging their application should see detailed information
    client
        .peer()
        .set_level(SetLevelRequestParam {
            level: LoggingLevel::Debug,
        })
        .await?;
    receive_signal.notified().await;
    {
        let messages = received_messages.lock().unwrap();
        let msg = messages.last().unwrap();
        let data = msg.data.as_object().unwrap();
        assert!(
            data.contains_key("function"),
            "Debug should show function name"
        );
        assert!(data.contains_key("line"), "Debug should show line number");
        assert!(
            data.contains_key("context"),
            "Debug should show execution context"
        );
    }

    // Test 3: Production monitoring scenario
    // User monitoring production should see important status updates
    client
        .peer()
        .set_level(SetLevelRequestParam {
            level: LoggingLevel::Info,
        })
        .await?;
    receive_signal.notified().await;
    {
        let messages = received_messages.lock().unwrap();
        let msg = messages.last().unwrap();
        let data = msg.data.as_object().unwrap();
        assert!(data.contains_key("status"), "Should show system status");
        assert!(data.contains_key("metrics"), "Should include metrics");
    }

    client.cancel().await?;
    Ok(())
}

#[test]
fn test_logging_level_serialization() {
    // Test all levels match spec exactly
    let test_cases = [
        (LoggingLevel::Alert, "alert"),
        (LoggingLevel::Critical, "critical"),
        (LoggingLevel::Debug, "debug"),
        (LoggingLevel::Emergency, "emergency"),
        (LoggingLevel::Error, "error"),
        (LoggingLevel::Info, "info"),
        (LoggingLevel::Notice, "notice"),
        (LoggingLevel::Warning, "warning"),
    ];

    for (level, expected) in test_cases {
        let serialized = serde_json::to_string(&level).unwrap();
        // Remove quotes from serialized string
        let serialized = serialized.trim_matches('"');
        assert_eq!(
            serialized, expected,
            "LoggingLevel::{:?} should serialize to \"{}\"",
            level, expected
        );
    }

    // Test deserialization from spec strings
    for (level, spec_string) in test_cases {
        let deserialized: LoggingLevel =
            serde_json::from_str(&format!("\"{}\"", spec_string)).unwrap();
        assert_eq!(
            deserialized, level,
            "\"{}\" should deserialize to LoggingLevel::{:?}",
            spec_string, level
        );
    }
}
