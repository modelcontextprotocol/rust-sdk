//cargo test --test test_close_connection --features "client server"

mod common;
use std::time::Duration;

use common::handlers::{TestClientHandler, TestServer};
use rmcp::{ServiceExt, service::QuitReason};

/// Test that close() properly shuts down the connection
#[tokio::test]
async fn test_close_method() -> anyhow::Result<()> {
    let (server_transport, client_transport) = tokio::io::duplex(4096);

    // Start server
    let server_handle = tokio::spawn(async move {
        let server = TestServer::new().serve(server_transport).await?;
        server.waiting().await?;
        anyhow::Ok(())
    });

    // Start client
    let handler = TestClientHandler::new(true, true);
    let mut client = handler.serve(client_transport).await?;

    // Verify client is not closed
    assert!(!client.is_closed());

    // Call close() and verify it returns
    let result = client.close().await?;
    assert!(matches!(result, QuitReason::Cancelled));

    // Verify client is now closed
    assert!(client.is_closed());

    // Calling close() again should return Closed immediately
    let result = client.close().await?;
    assert!(matches!(result, QuitReason::Closed));

    // Wait for server to finish
    server_handle.await??;
    Ok(())
}

/// Test that close_with_timeout() respects the timeout
#[tokio::test]
async fn test_close_with_timeout() -> anyhow::Result<()> {
    let (server_transport, client_transport) = tokio::io::duplex(4096);

    // Start server
    let server_handle = tokio::spawn(async move {
        let server = TestServer::new().serve(server_transport).await?;
        server.waiting().await?;
        anyhow::Ok(())
    });

    // Start client
    let handler = TestClientHandler::new(true, true);
    let mut client = handler.serve(client_transport).await?;

    // Close with a reasonable timeout
    let result = client.close_with_timeout(Duration::from_secs(5)).await?;
    assert!(result.is_some());
    assert!(matches!(result.unwrap(), QuitReason::Cancelled));

    // Verify client is now closed
    assert!(client.is_closed());

    // Wait for server to finish
    server_handle.await??;
    Ok(())
}

/// Test that cancel() still works and consumes self
#[tokio::test]
async fn test_cancel_method() -> anyhow::Result<()> {
    let (server_transport, client_transport) = tokio::io::duplex(4096);

    // Start server
    let server_handle = tokio::spawn(async move {
        let server = TestServer::new().serve(server_transport).await?;
        server.waiting().await?;
        anyhow::Ok(())
    });

    // Start client
    let handler = TestClientHandler::new(true, true);
    let client = handler.serve(client_transport).await?;

    // Cancel should work as before
    let result = client.cancel().await?;
    assert!(matches!(result, QuitReason::Cancelled));

    // Wait for server to finish
    server_handle.await??;
    Ok(())
}

/// Test that dropping without close() logs a debug message (we can't easily test
/// the log output, but we can verify the drop doesn't panic)
#[tokio::test]
async fn test_drop_without_close() -> anyhow::Result<()> {
    let (server_transport, client_transport) = tokio::io::duplex(4096);

    // Start server that will handle the drop
    let server_handle = tokio::spawn(async move {
        let server = TestServer::new().serve(server_transport).await?;
        // The server should close when the client drops
        let result = server.waiting().await?;
        // Server should detect closure
        assert!(matches!(result, QuitReason::Closed | QuitReason::Cancelled));
        anyhow::Ok(())
    });

    // Create and immediately drop the client
    {
        let handler = TestClientHandler::new(true, true);
        let _client = handler.serve(client_transport).await?;
        // Client dropped here without calling close()
    }

    // Give the async cleanup a moment to run
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Wait for server to finish (it should detect the closure)
    server_handle.await??;
    Ok(())
}
