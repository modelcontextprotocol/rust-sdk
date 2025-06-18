use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use anyhow::Result;
use clap::{Parser, ValueEnum};
use rmcp::{
    ClientHandler, ServiceExt,
    model::{
        CallToolRequestParam, ClientCapabilities, ClientInfo, Implementation,
        ProgressNotificationParam,
    },
    service::{NotificationContext, RoleClient},
    transport::{SseClientTransport, StreamableHttpClientTransport, TokioChildProcess},
};
use tokio::{process::Command, time::sleep};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[derive(Debug, Clone, ValueEnum)]
enum TransportType {
    Stdio,
    Sse,
    Http,
    All,
}

#[derive(Parser)]
#[command(name = "progress-test-client")]
#[command(about = "RMCP Progress Notification Test Client")]
struct Args {
    /// Transport type to test
    #[arg(short, long, value_enum, default_value_t = TransportType::Stdio)]
    transport: TransportType,

    /// Number of records to process
    #[arg(short, long, default_value_t = 10)]
    records: u32,

    /// SSE server URL
    #[arg(long, default_value = "http://127.0.0.1:8000/sse")]
    sse_url: String,

    /// HTTP server URL
    #[arg(long, default_value = "http://127.0.0.1:8001/mcp")]
    http_url: String,
}

#[derive(Debug, Clone)]
struct ProgressTracker {
    start_time: Instant,
    progress_count: Arc<Mutex<u32>>,
}

impl ProgressTracker {
    fn new() -> Self {
        Self {
            start_time: Instant::now(),
            progress_count: Arc::new(Mutex::new(0)),
        }
    }

    fn handle_progress(&self, params: &ProgressNotificationParam) {
        if let Ok(mut count) = self.progress_count.lock() {
            *count += 1;
        }
        let elapsed = self.start_time.elapsed();

        tracing::info!(
            "Progress update [{}]: {}/{} - {} (elapsed: {:.1}s)",
            params.progress_token.0,
            params.progress,
            params.total.unwrap_or(0.0),
            params.message.as_deref().unwrap_or(""),
            elapsed.as_secs_f64()
        );
    }

    fn print_summary(&self) {
        let elapsed = self.start_time.elapsed();
        if let Ok(count) = self.progress_count.lock() {
            tracing::info!("Total progress notifications received: {}", *count);
        }
        tracing::info!("Total time elapsed: {:.2}s", elapsed.as_secs_f64());
    }
}

// Progress-aware client handler
#[derive(Debug, Clone)]
struct ProgressAwareClient {
    tracker: Arc<Mutex<Option<ProgressTracker>>>,
}

impl ProgressAwareClient {
    fn new() -> Self {
        Self {
            tracker: Arc::new(Mutex::new(None)),
        }
    }

    fn start_tracking(&self) {
        if let Ok(mut tracker) = self.tracker.lock() {
            *tracker = Some(ProgressTracker::new());
        }
    }

    fn stop_tracking(&self) {
        if let Ok(mut tracker_opt) = self.tracker.lock() {
            if let Some(tracker) = tracker_opt.take() {
                tracker.print_summary();
            }
        }
    }
}

impl ClientHandler for ProgressAwareClient {
    async fn on_progress(
        &self,
        params: ProgressNotificationParam,
        _context: NotificationContext<RoleClient>,
    ) {
        if let Ok(tracker_opt) = self.tracker.lock() {
            if let Some(tracker) = tracker_opt.as_ref() {
                tracker.handle_progress(&params);
            }
        }
    }

    fn get_info(&self) -> ClientInfo {
        ClientInfo {
            protocol_version: Default::default(),
            capabilities: ClientCapabilities::default(),
            client_info: Implementation {
                name: "progress-test-client".to_string(),
                version: "1.0.0".to_string(),
                ..Default::default()
            },
        }
    }
}

async fn test_stdio_transport(records: u32) -> Result<()> {
    tracing::info!("Testing STDIO Transport");
    tracing::info!("================================");

    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let workspace_root = std::path::Path::new(manifest_dir)
        .parent()
        .and_then(|p| p.parent())
        .ok_or_else(|| anyhow::anyhow!("Cannot find workspace root"))?;

    let servers_dir = workspace_root.join("examples").join("servers");

    // Start server process
    let mut server_cmd = Command::new("cargo");
    server_cmd
        .current_dir(servers_dir)
        .arg("run")
        .arg("--example")
        .arg("servers_progress_demo")
        .arg("--")
        .arg("stdio");

    // Create progress-aware client handler
    let client_handler = ProgressAwareClient::new();
    client_handler.start_tracking();
    let client_handler_clone = client_handler.clone();

    let service = client_handler
        .serve(TokioChildProcess::new(server_cmd)?)
        .await?;

    // Initialize
    let server_info = service.peer_info();
    if let Some(info) = server_info {
        tracing::info!("Connected to server: {:?}", info.server_info.name);
    }

    // List tools
    let tools = service.list_all_tools().await?;
    tracing::info!(
        "Available tools: {:?}",
        tools.iter().map(|t| &t.name).collect::<Vec<_>>()
    );

    // Call stream processor tool
    tracing::info!("Starting to process {} records...", records);
    let tool_result = service
        .call_tool(CallToolRequestParam {
            name: "stream_processor".into(),
            arguments: None,
        })
        .await?;

    if let Some(content) = tool_result.content.first() {
        if let Some(text) = content.as_text() {
            tracing::info!("Processing completed: {}", text.text);
        }
    }

    service.cancel().await?;
    client_handler_clone.stop_tracking();
    tracing::info!("STDIO transport test completed successfully!");
    Ok(())
}
// Test SSE transport, must run the server with `cargo run --example servers_progress_demo -- sse` in the servers directory
async fn test_sse_transport(sse_url: &str, records: u32) -> Result<()> {
    tracing::info!("Testing SSE Transport");
    tracing::info!("=========================");
    tracing::info!("SSE URL: {}", sse_url);

    // Wait a bit for server to be ready
    sleep(Duration::from_secs(1)).await;

    let transport = SseClientTransport::start(sse_url).await?;

    // Create progress-aware client handler
    let client_handler = ProgressAwareClient::new();
    client_handler.start_tracking();
    let client_handler_clone = client_handler.clone();

    let client = client_handler.serve(transport).await.inspect_err(|e| {
        tracing::error!("SSE client error: {:?}", e);
    })?;

    // Initialize
    let server_info = client.peer_info();
    if let Some(info) = server_info {
        tracing::info!("Connected to server: {:?}", info.server_info.name);
    }

    // List tools
    let tools = client.list_tools(Default::default()).await?;
    tracing::info!(
        "Available tools: {:?}",
        tools.tools.iter().map(|t| &t.name).collect::<Vec<_>>()
    );

    // Call stream processor tool
    tracing::info!("Starting to process {} records...", records);
    let tool_result = client
        .call_tool(CallToolRequestParam {
            name: "stream_processor".into(),
            arguments: None,
        })
        .await?;

    if let Some(content) = tool_result.content.first() {
        if let Some(text) = content.as_text() {
            tracing::info!("Processing completed: {}", text.text);
        }
    }

    client.cancel().await?;
    client_handler_clone.stop_tracking();
    tracing::info!("SSE transport test completed successfully!");
    Ok(())
}
// Test HTTP transport, must run the server with `cargo run --example servers_progress_demo -- http` in the servers directory
async fn test_http_transport(http_url: &str, records: u32) -> Result<()> {
    tracing::info!("Testing HTTP Streaming Transport");
    tracing::info!("=====================================");
    tracing::info!("HTTP URL: {}", http_url);

    // Wait a bit for server to be ready
    sleep(Duration::from_secs(1)).await;

    let transport = StreamableHttpClientTransport::from_uri(http_url);

    // Create progress-aware client handler
    let client_handler = ProgressAwareClient::new();
    client_handler.start_tracking();
    let client_handler_clone = client_handler.clone();

    let client = client_handler.serve(transport).await.inspect_err(|e| {
        tracing::error!("HTTP client error: {:?}", e);
    })?;

    // Initialize
    let server_info = client.peer_info();
    if let Some(info) = server_info {
        tracing::info!("Connected to server: {:?}", info.server_info.name);
    }

    // List tools
    let tools = client.list_tools(Default::default()).await?;
    tracing::info!(
        "Available tools: {:?}",
        tools.tools.iter().map(|t| &t.name).collect::<Vec<_>>()
    );

    // Call stream processor tool
    tracing::info!("Starting to process {} records...", records);
    let tool_result = client
        .call_tool(CallToolRequestParam {
            name: "stream_processor".into(),
            arguments: None,
        })
        .await?;

    if let Some(content) = tool_result.content.first() {
        if let Some(text) = content.as_text() {
            tracing::info!("processing completed: {}", text.text);
        }
    }

    client.cancel().await?;
    client_handler_clone.stop_tracking();
    tracing::info!("HTTP transport test completed successfully!");
    Ok(())
}

async fn run_single_test(transport_type: &TransportType, args: &Args) -> Result<bool> {
    match transport_type {
        TransportType::Stdio => {
            test_stdio_transport(args.records).await?;
            Ok(true)
        }
        TransportType::Sse => match test_sse_transport(&args.sse_url, args.records).await {
            Ok(_) => Ok(true),
            Err(e) => {
                tracing::error!("SSE test failed: {}", e);
                Ok(false)
            }
        },
        TransportType::Http => match test_http_transport(&args.http_url, args.records).await {
            Ok(_) => Ok(true),
            Err(e) => {
                tracing::error!("HTTP test failed: {}", e);
                Ok(false)
            }
        },
        TransportType::All => {
            // This case is handled in main
            Ok(true)
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Initialize logging
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    tracing::info!("RMCP Progress Notification Test Client");
    tracing::info!("==========================================");

    match &args.transport {
        TransportType::All => {
            // Test all transport types
            let mut results = std::collections::HashMap::new();

            for transport_type in [
                TransportType::Stdio,
                TransportType::Sse,
                TransportType::Http,
            ] {
                let transport_name = format!("{:?}", transport_type).to_uppercase();
                tracing::info!("\n");

                match run_single_test(&transport_type, &args).await {
                    Ok(success) => {
                        results.insert(transport_name, success);
                    }
                    Err(e) => {
                        tracing::error!("{} test failed: {}", transport_name, e);
                        results.insert(transport_name, false);
                    }
                }

                sleep(Duration::from_secs(2)).await;
            }

            // Print summary
            tracing::info!("\n==========================================");
            tracing::info!("Test Results Summary");
            tracing::info!("==========================================");

            for (transport, success) in &results {
                let status = if *success { "PASSED" } else { "FAILED" };
                tracing::info!("  {:<10}: {}", transport, status);
            }
        }
        transport_type => {
            run_single_test(transport_type, &args).await?;
        }
    }

    Ok(())
}
