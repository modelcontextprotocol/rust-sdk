//! build: cargo build -p mcp-server-examples --example wasi_std_io --target wasm32-wasip1
//!
//! run: npx @modelcontextprotocol/inspector wasmedge --dir logs:. run target/wasm32-wasip1/debug/examples/wasi_std_io.wasm
//!
use mcp_server::{router::RouterService, ByteTransport, Server};
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::EnvFilter;
mod common;
use anyhow::Result;
use common::counter::CounterRouter;
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    // Set up file appender for logging
    let file_appender = RollingFileAppender::new(Rotation::DAILY, "logs", "mcp-server.log");

    // Initialize the tracing subscriber with file and stdout logging
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive(tracing::Level::INFO.into()))
        .with_writer(file_appender)
        .with_target(false)
        .with_thread_ids(true)
        .with_file(true)
        .with_line_number(true)
        .init();

    tracing::info!("Starting MCP server");

    // Create an instance of our counter router
    let router = RouterService(CounterRouter::new());

    // Create and run the server
    let server = Server::new(router);
    #[cfg(target_arch = "wasm32")]
    let transport = ByteTransport::new(async_io::WasiFd::std_in(), async_io::WasiFd::std_out());
    #[cfg(not(target_arch = "wasm32"))]
    let transport = ByteTransport::new(tokio::io::stdin(), tokio::io::stdout());

    tracing::info!("Server initialized and ready to handle requests");
    Ok(server.run(transport).await?)
}

#[cfg(target_arch = "wasm32")]
mod async_io {
    use tokio::io::{AsyncRead, AsyncWrite};
    use wasi::{Fd, FD_STDIN, FD_STDOUT};

    pub struct WasiFd {
        fd: Fd,
    }

    impl WasiFd {
        pub fn std_in() -> Self {
            Self { fd: FD_STDIN }
        }
        pub fn std_out() -> Self {
            Self { fd: FD_STDOUT }
        }
    }

    impl AsyncRead for WasiFd {
        fn poll_read(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            let mut temp_buf = vec![0u8; buf.remaining()];
            unsafe {
                match wasi::fd_read(
                    self.fd,
                    &[wasi::Iovec {
                        buf: temp_buf.as_mut_ptr(),
                        buf_len: temp_buf.len(),
                    }],
                ) {
                    Ok(n) => {
                        buf.put_slice(&temp_buf[..n]);
                        std::task::Poll::Ready(Ok(()))
                    }
                    Err(err) => std::task::Poll::Ready(Err(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        format!("WASI read error: {}", err),
                    ))),
                }
            }
        }
    }

    impl AsyncWrite for WasiFd {
        fn poll_write(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            buf: &[u8],
        ) -> std::task::Poll<Result<usize, std::io::Error>> {
            unsafe {
                match wasi::fd_write(
                    self.fd,
                    &[wasi::Ciovec {
                        buf: buf.as_ptr(),
                        buf_len: buf.len(),
                    }],
                ) {
                    Ok(n) => std::task::Poll::Ready(Ok(n)),
                    Err(err) => std::task::Poll::Ready(Err(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        format!("WASI write error: {}", err),
                    ))),
                }
            }
        }

        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), std::io::Error>> {
            std::task::Poll::Ready(Ok(()))
        }

        fn poll_shutdown(
            self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), std::io::Error>> {
            self.poll_flush(cx)
        }
    }
}
