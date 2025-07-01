#![cfg(feature = "transport-sse-server")]

// Import framework-specific types
#[cfg(feature = "actix-web")]
use rmcp::transport::sse_server::actix_web::SseServer as ActixSseServer;
#[cfg(feature = "axum")]
use rmcp::transport::sse_server::axum::SseServer as AxumSseServer;
use rmcp::{ServiceExt, transport::sse_server::SseServerConfig};
use tokio_util::sync::CancellationToken;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod common;
use common::calculator::Calculator;

async fn init() {
    let _ = tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "debug".to_string().into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .try_init();
}

// Common test logic for basic SSE server test
async fn test_sse_server_basic_common(
    bind_addr: std::net::SocketAddr,
    ct: CancellationToken,
    service_ct: CancellationToken,
) -> anyhow::Result<()> {
    // Give the server a moment to start
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Test that server is running by making a request
    let client = reqwest::Client::new();
    let response = client
        .get(format!("http://{}/sse", bind_addr))
        .header("Accept", "text/event-stream")
        .send()
        .await?;

    // SSE endpoint should return OK and start streaming
    assert_eq!(response.status(), reqwest::StatusCode::OK);

    ct.cancel();
    service_ct.cancel();
    Ok(())
}

#[cfg(all(feature = "transport-sse-server", feature = "axum"))]
#[tokio::test]
async fn test_axum_sse_server_basic() -> anyhow::Result<()> {
    init().await;

    let config = SseServerConfig {
        bind: "127.0.0.1:0".parse()?,
        sse_path: "/sse".to_string(),
        post_path: "/message".to_string(),
        ct: CancellationToken::new(),
        sse_keep_alive: None,
    };

    let ct = config.ct.clone();
    let sse_server = AxumSseServer::serve_with_config(config).await?;
    let bind_addr = sse_server.config.bind;
    let service_ct = sse_server.with_service(Calculator::default);

    test_sse_server_basic_common(bind_addr, ct, service_ct).await
}

#[cfg(all(feature = "transport-sse-server", feature = "actix-web"))]
#[actix_web::test]
async fn test_actix_sse_server_basic() -> anyhow::Result<()> {
    init().await;

    let config = SseServerConfig {
        bind: "127.0.0.1:0".parse()?,
        sse_path: "/sse".to_string(),
        post_path: "/message".to_string(),
        ct: CancellationToken::new(),
        sse_keep_alive: None,
    };

    let ct = config.ct.clone();
    let sse_server = ActixSseServer::serve_with_config(config).await?;
    let bind_addr = sse_server.config.bind;
    let service_ct = sse_server.with_service(Calculator::default);

    test_sse_server_basic_common(bind_addr, ct, service_ct).await
}

// Common client-server integration test logic
#[cfg(feature = "transport-sse-client")]
async fn test_client_server_integration_common(
    actual_addr: std::net::SocketAddr,
    ct: CancellationToken,
) -> anyhow::Result<()> {
    use rmcp::transport::SseClientTransport;

    let transport = SseClientTransport::start(format!("http://{}/sse", actual_addr)).await?;
    let client = ().serve(transport).await?;

    // Test basic operations
    let tools = client.list_all_tools().await?;
    assert!(!tools.is_empty());
    assert_eq!(tools.len(), 2); // sum and sub

    client.cancel().await?;
    ct.cancel();
    Ok(())
}

#[cfg(all(
    feature = "transport-sse-server",
    feature = "transport-sse-client",
    feature = "axum"
))]
#[tokio::test]
async fn test_axum_client_server_integration() -> anyhow::Result<()> {
    init().await;

    const BIND_ADDRESS: &str = "127.0.0.1:0";

    let sse_server = AxumSseServer::serve(BIND_ADDRESS.parse()?).await?;
    let actual_addr = sse_server.config.bind;
    let ct = sse_server.with_service(Calculator::default);

    test_client_server_integration_common(actual_addr, ct).await
}

#[cfg(all(
    feature = "transport-sse-server",
    feature = "transport-sse-client",
    feature = "actix-web"
))]
#[actix_web::test]
async fn test_actix_client_server_integration() -> anyhow::Result<()> {
    init().await;

    const BIND_ADDRESS: &str = "127.0.0.1:0";

    let sse_server = ActixSseServer::serve(BIND_ADDRESS.parse()?).await?;
    let actual_addr = sse_server.config.bind;
    let ct = sse_server.with_service(Calculator::default);

    // Give the server a moment to start
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    test_client_server_integration_common(actual_addr, ct).await
}

// Common concurrent clients test logic
#[cfg(feature = "transport-sse-client")]
async fn test_concurrent_clients_common(
    actual_addr: std::net::SocketAddr,
    ct: CancellationToken,
) -> anyhow::Result<()> {
    use rmcp::transport::SseClientTransport;

    const NUM_CLIENTS: usize = 5;
    let mut handles = vec![];

    for i in 0..NUM_CLIENTS {
        let addr = actual_addr;
        let handle = tokio::spawn(async move {
            let transport = SseClientTransport::start(format!("http://{}/sse", addr)).await?;
            let client = ().serve(transport).await?;

            // Each client does some operations
            let tools = client.list_all_tools().await?;
            assert!(!tools.is_empty());
            assert_eq!(tools.len(), 2); // sum and sub

            tracing::info!("Client {} completed operations", i);
            client.cancel().await?;
            Ok::<(), anyhow::Error>(())
        });
        handles.push(handle);
    }

    // Wait for all clients to complete
    for handle in handles {
        handle.await??;
    }

    ct.cancel();
    Ok(())
}

#[cfg(all(
    feature = "transport-sse-server",
    feature = "transport-sse-client",
    feature = "axum"
))]
#[tokio::test]
async fn test_axum_concurrent_clients() -> anyhow::Result<()> {
    init().await;

    const BIND_ADDRESS: &str = "127.0.0.1:0";

    let sse_server = AxumSseServer::serve(BIND_ADDRESS.parse()?).await?;
    let actual_addr = sse_server.config.bind;
    let ct = sse_server.with_service(Calculator::default);

    test_concurrent_clients_common(actual_addr, ct).await
}

#[cfg(all(
    feature = "transport-sse-server",
    feature = "transport-sse-client",
    feature = "actix-web"
))]
#[actix_web::test]
async fn test_actix_concurrent_clients() -> anyhow::Result<()> {
    init().await;

    const BIND_ADDRESS: &str = "127.0.0.1:0";

    let sse_server = ActixSseServer::serve(BIND_ADDRESS.parse()?).await?;
    let actual_addr = sse_server.config.bind;
    let ct = sse_server.with_service(Calculator::default);

    // Give the server a moment to start
    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    test_concurrent_clients_common(actual_addr, ct).await
}
