use rmcp::{
    ClientHandler, Peer, RoleClient, ServerHandler, ServiceExt,
    model::{
        ServerCapabilities, ServerInfo, SetLevelRequestParam,
        LoggingMessageNotificationParam, LoggingLevel,
    },
    service::RequestContext,
    RoleServer,
    Error as McpError,
};

use std::sync::Arc;
use tokio::sync::Notify;
use std::sync::Mutex;
use serde_json;
use chrono;
use std::future::Future;

pub struct LoggingClient {
    receive_signal: Arc<Notify>,
    received_messages: Arc<Mutex<Vec<LoggingMessageNotificationParam>>>,
    peer: Option<Peer<RoleClient>>,
}

impl ClientHandler for LoggingClient {
    async fn on_logging_message(&self, params: LoggingMessageNotificationParam) {
        println!("Client: Received log message: {:?}", params);
        self.received_messages.lock().unwrap().push(params);
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

impl ServerHandler for TestServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            capabilities: ServerCapabilities::builder()
                .enable_logging()
                .build(),
            ..Default::default()
        }
    }

    fn set_level(
        &self,
        request: SetLevelRequestParam,
        context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<(), McpError>> + Send + '_ {
        std::future::ready(Ok(()))  // Just accept the level setting
    }
}

#[tokio::test]
async fn test_logging() -> anyhow::Result<()> {
    let (server_transport, client_transport) = tokio::io::duplex(4096);
    let receive_signal = Arc::new(Notify::new());
    let received_messages = Arc::new(Mutex::new(Vec::new()));

    // Start server first, but just waiting
    tokio::spawn(async move {
        let server = TestServer {}.serve(server_transport).await?;
        
        // Send test message after server is ready
        server.peer().notify_logging_message(LoggingMessageNotificationParam {
            level: LoggingLevel::Info,
            data: serde_json::json!({
                "message": "Test message",
                "timestamp": chrono::Utc::now().to_rfc3339(),
            }),
            logger: None,
        }).await?;
        
        server.waiting().await?;
        anyhow::Ok(())
    });

    // Setup client
    let client = LoggingClient {
        receive_signal: receive_signal.clone(),
        received_messages: received_messages.clone(),
        peer: None,
    }
    .serve(client_transport)
    .await?;

    // Set level to Info to ensure we receive messages
    client.peer().set_level(SetLevelRequestParam {
        level: LoggingLevel::Info,
    }).await?;

    // Wait for message
    receive_signal.notified().await;

    // Verify message format
    let messages = received_messages.lock().unwrap();
    assert_eq!(messages.len(), 1);
    
    let msg = &messages[0];
    assert_eq!(msg.level, LoggingLevel::Info, "Message should be at Info level");
    
    let data = msg.data.as_object().expect("data should be an object");
    assert!(data.contains_key("message"), "Message missing message field");
    assert!(data.contains_key("timestamp"), "Message missing timestamp field");

    // Cleanup
    client.cancel().await?;
    
    Ok(())
}

// Helper function to convert LoggingLevel to numeric value for comparison
fn level_to_number(level: LoggingLevel) -> u8 {
    match level {
        LoggingLevel::Debug => 0,
        LoggingLevel::Info => 1,
        LoggingLevel::Notice => 2,
        LoggingLevel::Warning => 3,
        LoggingLevel::Error => 4,
        LoggingLevel::Critical => 5,
        LoggingLevel::Alert => 6,
        LoggingLevel::Emergency => 7,
    }
}