use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// # StdIO Transport
///
/// Create a pair of [`tokio::io::Stdin`] and [`tokio::io::Stdout`].
///
/// Every read from and write to these handles is executed on Tokio's blocking
/// thread pool. When both standard streams are pipes, prefer [`stdio_pipes`],
/// which avoids the per-operation thread hand-off on Unix.
pub fn stdio() -> (tokio::io::Stdin, tokio::io::Stdout) {
    (tokio::io::stdin(), tokio::io::stdout())
}

/// # Non-blocking StdIO Transport
///
/// Like [`stdio`], but uses Tokio's non-blocking Unix pipe driver when both
/// standard streams are FIFOs — the way MCP clients launch stdio servers — so
/// reads and writes are driven by the IO reactor instead of Tokio's blocking
/// thread pool. This avoids a cross-thread hand-off per message under
/// concurrent requests.
///
/// Falls back to the blocking [`stdio`] handles when the streams are not
/// pipes (TTYs, files, sockets) or on platforms without Unix pipes, so it can
/// be used unconditionally.
///
/// On Unix the returned reader and writer own descriptors duplicated from
/// standard input and output; closing them does not close the process's
/// standard streams.
///
/// # Panics
///
/// On Unix, panics if called outside of a Tokio runtime with IO enabled,
/// mirroring `tokio::net::unix::pipe::Receiver::from_owned_fd`.
pub fn stdio_pipes() -> (StdioReader, StdioWriter) {
    #[cfg(unix)]
    if let Some(pair) = try_pipe_transport() {
        return pair;
    }
    (
        StdioReader::Blocking(tokio::io::stdin()),
        StdioWriter::Blocking(tokio::io::stdout()),
    )
}

#[cfg(unix)]
fn try_pipe_transport() -> Option<(StdioReader, StdioWriter)> {
    use std::os::fd::AsFd;

    let read = std::io::stdin().as_fd().try_clone_to_owned().ok()?;
    let write = std::io::stdout().as_fd().try_clone_to_owned().ok()?;
    pipe_transport(read, write)
}

/// Wraps two FIFO descriptors as a non-blocking pipe transport.
///
/// Returns `None` unless both descriptors are FIFOs. Both are checked before
/// either is handed to Tokio: `pipe::Receiver::from_owned_fd` and
/// `pipe::Sender::from_owned_fd` put the descriptor into non-blocking mode,
/// and `O_NONBLOCK` is shared by every descriptor of the same open file
/// description, so a partial conversion would leave the blocking fallback
/// with non-blocking stdio.
#[cfg(unix)]
fn pipe_transport(
    read: std::os::fd::OwnedFd,
    write: std::os::fd::OwnedFd,
) -> Option<(StdioReader, StdioWriter)> {
    if !is_fifo(&read) || !is_fifo(&write) {
        return None;
    }
    let read = tokio::net::unix::pipe::Receiver::from_owned_fd(read).ok()?;
    let write = tokio::net::unix::pipe::Sender::from_owned_fd(write).ok()?;
    Some((StdioReader::Pipe(read), StdioWriter::Pipe(write)))
}

#[cfg(unix)]
fn is_fifo(fd: &std::os::fd::OwnedFd) -> bool {
    use std::os::unix::fs::FileTypeExt;

    fd.try_clone()
        .map(std::fs::File::from)
        .and_then(|file| file.metadata())
        .is_ok_and(|metadata| metadata.file_type().is_fifo())
}

/// Reader half of the transport returned by [`stdio_pipes`].
#[non_exhaustive]
pub enum StdioReader {
    /// Blocking standard input, as returned by [`stdio`].
    Blocking(tokio::io::Stdin),
    /// Non-blocking standard input backed by a Unix pipe.
    #[cfg(unix)]
    Pipe(tokio::net::unix::pipe::Receiver),
}

impl AsyncRead for StdioReader {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Blocking(reader) => Pin::new(reader).poll_read(cx, buf),
            #[cfg(unix)]
            Self::Pipe(reader) => Pin::new(reader).poll_read(cx, buf),
        }
    }
}

/// Writer half of the transport returned by [`stdio_pipes`].
#[non_exhaustive]
pub enum StdioWriter {
    /// Blocking standard output, as returned by [`stdio`].
    Blocking(tokio::io::Stdout),
    /// Non-blocking standard output backed by a Unix pipe.
    #[cfg(unix)]
    Pipe(tokio::net::unix::pipe::Sender),
}

impl AsyncWrite for StdioWriter {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Blocking(writer) => Pin::new(writer).poll_write(cx, buf),
            #[cfg(unix)]
            Self::Pipe(writer) => Pin::new(writer).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Blocking(writer) => Pin::new(writer).poll_flush(cx),
            #[cfg(unix)]
            Self::Pipe(writer) => Pin::new(writer).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Blocking(writer) => Pin::new(writer).poll_shutdown(cx),
            #[cfg(unix)]
            Self::Pipe(writer) => Pin::new(writer).poll_shutdown(cx),
        }
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Blocking(writer) => Pin::new(writer).poll_write_vectored(cx, bufs),
            #[cfg(unix)]
            Self::Pipe(writer) => Pin::new(writer).poll_write_vectored(cx, bufs),
        }
    }

    fn is_write_vectored(&self) -> bool {
        match self {
            Self::Blocking(writer) => writer.is_write_vectored(),
            #[cfg(unix)]
            Self::Pipe(writer) => writer.is_write_vectored(),
        }
    }
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    fn fifo_pair() -> (
        std::process::Child,
        std::os::fd::OwnedFd,
        std::os::fd::OwnedFd,
    ) {
        use std::process::{Command, Stdio};

        let mut child = Command::new("cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("failed to spawn `cat`");
        let write = child.stdin.take().expect("child stdin").into();
        let read = child.stdout.take().expect("child stdout").into();
        (child, read, write)
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn pipe_transport_round_trips_over_fifos() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        use super::{StdioReader, StdioWriter, pipe_transport};

        let (mut child, read, write) = fifo_pair();
        let (mut reader, mut writer) = pipe_transport(read, write).expect("child pipes are FIFOs");
        assert!(matches!(reader, StdioReader::Pipe(_)));
        assert!(matches!(writer, StdioWriter::Pipe(_)));

        let message = b"{\"jsonrpc\":\"2.0\",\"id\":1}\n";
        writer.write_all(message).await.unwrap();
        writer.flush().await.unwrap();
        let mut echoed = vec![0; message.len()];
        reader.read_exact(&mut echoed).await.unwrap();
        assert_eq!(echoed, message);

        drop(writer);
        assert!(child.wait().unwrap().success());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn pipe_transport_falls_back_for_non_fifos() {
        use super::pipe_transport;

        let read = std::fs::File::open("/dev/null").unwrap().into();
        let write = std::fs::File::open("/dev/null").unwrap().into();
        assert!(pipe_transport(read, write).is_none());
    }

    #[cfg(feature = "server")]
    #[tokio::test]
    async fn stdio_pipes_builds_an_async_rw_transport() {
        use super::stdio_pipes;
        use crate::{RoleServer, transport::async_rw::AsyncRwTransport};

        let (reader, writer) = stdio_pipes();
        let _transport: AsyncRwTransport<RoleServer, _, _> = AsyncRwTransport::new(reader, writer);
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn stdio_pipes_falls_back_to_blocking_handles_on_windows() {
        use super::{StdioReader, StdioWriter, stdio_pipes};

        let (reader, writer) = stdio_pipes();
        assert!(matches!(reader, StdioReader::Blocking(_)));
        assert!(matches!(writer, StdioWriter::Blocking(_)));
    }
}
