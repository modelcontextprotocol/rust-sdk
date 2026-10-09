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
    ClientHandler, ErrorData, RoleClient, ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolRequestParams, ClientConfig, ServerCapabilities, ServerConfig},
    service::{QuitReason, RequestContext, serve_directly_with_ct},
    tool, tool_handler, tool_router,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt, ReadBuf},
    sync::mpsc,
};
use tokio_util::sync::CancellationToken;

// A slow tool server that sleeps before returning a response.
#[derive(Debug, Clone)]
struct SlowToolServer {
    #[expect(dead_code, reason = "tool_handler macro accesses this router field")]
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
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
    }
}

#[derive(Debug, Clone, Default)]
struct DummyClientHandler;

impl rmcp::ClientHandler for DummyClientHandler {
    fn get_info(&self) -> ClientConfig {
        ClientConfig::default()
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
        .and_then(|c| c.as_text())
        .map(|t| t.text.as_str())
        .expect("expected text content in tool result");
    assert_eq!(text, "done after 200ms");

    server_handle.await??;
    Ok(())
}

struct PendingPingClient {
    started: mpsc::UnboundedSender<CancellationToken>,
    finished: mpsc::UnboundedSender<()>,
}

impl ClientHandler for PendingPingClient {
    async fn ping(&self, context: RequestContext<RoleClient>) -> Result<(), ErrorData> {
        self.started.send(context.ct.clone()).unwrap();
        context.ct.cancelled().await;
        self.finished.send(()).unwrap();
        Ok(())
    }
}

#[tokio::test(start_paused = true)]
async fn test_eof_cancels_pending_requests_after_drain() -> anyhow::Result<()> {
    let (mut remote_write, client_read) = tokio::io::duplex(4096);
    let (client_write, mut remote_read) = tokio::io::duplex(4096);
    let (started_tx, mut started_rx) = mpsc::unbounded_channel();
    let (finished_tx, mut finished_rx) = mpsc::unbounded_channel();
    let service_token = CancellationToken::new();
    let sibling_token = service_token.child_token();
    let client = serve_directly_with_ct::<RoleClient, _, _, _, _>(
        PendingPingClient {
            started: started_tx,
            finished: finished_tx,
        },
        (client_read, client_write),
        None,
        service_token.clone(),
    );
    let peer = client.peer().clone();

    remote_write
        .write_all(
            b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n\
              {\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\"}\n",
        )
        .await?;
    let first = started_rx.recv().await.expect("first handler started");
    let second = started_rx.recv().await.expect("second handler started");
    assert!(!first.is_cancelled());
    assert!(!second.is_cancelled());

    // EOF closes only the input side. Pending handlers keep the response drain
    // open until its deadline, which passes instantly with paused Tokio time.
    drop(remote_write);
    let mut responses = Vec::new();
    tokio::time::timeout(
        Duration::from_secs(6),
        remote_read.read_to_end(&mut responses),
    )
    .await??;

    // Keep RunningService and Peer alive through these assertions: dropping the
    // service would cancel its token and hide missing cancellation on EOF.
    assert!(peer.is_transport_closed());
    assert!(first.is_cancelled());
    assert!(second.is_cancelled());
    assert!(!service_token.is_cancelled());
    assert!(!sibling_token.is_cancelled());
    for _ in 0..2 {
        tokio::time::timeout(Duration::from_secs(1), finished_rx.recv())
            .await?
            .expect("handler finished after cancellation");
    }
    assert!(matches!(client.waiting().await?, QuitReason::Closed));
    Ok(())
}
