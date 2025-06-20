pub mod calculator;
use std::task::{Poll, Waker};

use rmcp::ServiceExt;
use tokio::io::{AsyncRead, AsyncWrite};
use tracing_subscriber::EnvFilter;
use wasi::{
    cli::{
        stdin::{InputStream, get_stdin},
        stdout::{OutputStream, get_stdout},
    },
    io::streams::Pollable,
};

pub fn wasi_io() -> (AsyncInputStream, AsyncOutputStream) {
    let input = AsyncInputStream { inner: get_stdin() };
    let output = AsyncOutputStream {
        inner: get_stdout(),
    };
    (input, output)
}

pub struct AsyncInputStream {
    inner: InputStream,
}

impl AsyncRead for AsyncInputStream {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let bytes = self
            .inner
            .read(buf.remaining() as u64)
            .map_err(std::io::Error::other)?;
        if bytes.is_empty() {
            let pollable = self.inner.subscribe();
            let waker = cx.waker().clone();
            runtime_poll(waker, pollable);
            return Poll::Pending;
        }
        buf.put_slice(&bytes);
        std::task::Poll::Ready(Ok(()))
    }
}

pub struct AsyncOutputStream {
    inner: OutputStream,
}
fn runtime_poll(waker: Waker, pollable: Pollable) {
    tokio::task::spawn(async move {
        loop {
            if pollable.ready() {
                waker.wake();
                break;
            } else {
                tokio::task::yield_now().await;
            }
        }
    });
}
impl AsyncWrite for AsyncOutputStream {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, std::io::Error>> {
        let writable_len = self.inner.check_write().map_err(std::io::Error::other)?;
        if writable_len == 0 {
            let pollable = self.inner.subscribe();
            let waker = cx.waker().clone();
            runtime_poll(waker, pollable);
            return Poll::Pending;
        }
        let bytes_to_write = buf.len().min(writable_len as usize);
        self.inner
            .write(&buf[0..bytes_to_write])
            .map_err(std::io::Error::other)?;
        Poll::Ready(Ok(bytes_to_write))
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        self.inner.flush().map_err(std::io::Error::other)?;
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        self.poll_flush(cx)
    }
}

struct TokioCliRunner;

impl wasi::exports::cli::run::Guest for TokioCliRunner {
    fn run() -> Result<(), ()> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            tracing_subscriber::fmt()
                .with_env_filter(
                    EnvFilter::from_default_env().add_directive(tracing::Level::DEBUG.into()),
                )
                .with_writer(std::io::stderr)
                .with_ansi(false)
                .init();
            let server = calculator::Calculator::new()
                .serve(wasi_io())
                .await
                .unwrap();
            server.waiting().await.unwrap();
        });
        Ok(())
    }
}
wasi::cli::command::export!(TokioCliRunner);
