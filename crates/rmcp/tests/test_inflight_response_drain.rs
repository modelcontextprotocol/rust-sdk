#![cfg(all(feature = "client", feature = "server", not(feature = "local")))]
// cargo test --test test_inflight_response_drain --features "client server"

use std::{
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll},
    time::Duration,
};

use rmcp::{
    ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolRequestParams, ClientInfo, ServerCapabilities, ServerInfo},
    service::QuitReason,
    tool, tool_handler, tool_router,
};
use tokio::io::{AsyncRead, ReadBuf};

// A slow tool server that sleeps before returning a response.
#[derive(Debug, Clone)]
struct SlowToolServer {
    tool_router: ToolRouter<Self>,
}

impl SlowToolServer {
    fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct SlowToolRequest {
    #[schemars(description = "how long to sleep in milliseconds")]
    sleep_ms: u64,
}

#[tool_router]
impl SlowToolServer {
    #[tool(description = "A tool that sleeps then returns")]
    async fn slow_tool(
        &self,
        Parameters(SlowToolRequest { sleep_ms }): Parameters<SlowToolRequest>,
    ) -> String {
        tokio::time::sleep(Duration::from_millis(sleep_ms)).await;
        format!("done after {}ms", sleep_ms)
    }
}

#[tool_handler]
impl ServerHandler for SlowToolServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
    }
}

#[derive(Debug, Clone, Default)]
struct DummyClientHandler;

impl rmcp::ClientHandler for DummyClientHandler {
    fn get_info(&self) -> ClientInfo {
        ClientInfo::default()
    }
}

/// An `AsyncRead` wrapper that delegates to the inner reader until signalled,
/// then returns EOF (read 0 bytes).
struct ClosableReader<R> {
    inner: R,
    eof_flag: Arc<AtomicBool>,
}

impl<R: AsyncRead + Unpin> AsyncRead for ClosableReader<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if self.eof_flag.load(Ordering::Acquire) {
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

/// When the server's input stream returns EOF while a tool handler is still
/// in-flight, the drain phase should flush pending responses before closing.
#[tokio::test]
async fn test_inflight_response_drain_on_eof() -> anyhow::Result<()> {
    // Two unidirectional channels:
    // client_write → server_read  (client sends requests to server)
    // server_write → client_read  (server sends responses to client)
    let (client_write, server_read) = tokio::io::duplex(4096);
    let (server_write, client_read) = tokio::io::duplex(4096);

    // Wrap the server's read side so we can signal EOF from the test.
    let eof_flag = Arc::new(AtomicBool::new(false));
    let closable_read = ClosableReader {
        inner: server_read,
        eof_flag: eof_flag.clone(),
    };

    let server_transport = (closable_read, server_write);
    let client_transport = (client_read, client_write);

    // Start server with slow tool handler
    let server_handle = tokio::spawn(async move {
        let server = SlowToolServer::new();
        let running = server.serve(server_transport).await?;
        let reason = running.waiting().await?;
        assert!(
            matches!(reason, QuitReason::Closed),
            "expected Closed quit reason, got {:?}",
            reason,
        );
        anyhow::Ok(())
    });

    // Start client
    let client = DummyClientHandler.serve(client_transport).await?;

    // Call the slow tool (200ms sleep). Concurrently, signal the server's
    // read side to return EOF after the request has been sent but before
    // the handler finishes.
    let tool_future = client.call_tool(
        CallToolRequestParams::new("slow_tool").with_arguments(
            serde_json::json!({ "sleep_ms": 200 })
                .as_object()
                .unwrap()
                .clone(),
        ),
    );

    let (tool_result, _) = tokio::join!(tool_future, async {
        // Wait for the request to be sent and received by the server,
        // then signal EOF on the server's read side.
        tokio::time::sleep(Duration::from_millis(50)).await;
        eof_flag.store(true, Ordering::Release);
    });

    // The tool result should still arrive thanks to the drain phase.
    let result = tool_result?;
    let text = result
        .content
        .first()
        .and_then(|c| c.raw.as_text())
        .map(|t| t.text.as_str())
        .expect("expected text content in tool result");
    assert_eq!(text, "done after 200ms");

    server_handle.await??;
    Ok(())
}
