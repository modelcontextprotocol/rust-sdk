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
/// which avoids the per-operation thread hand-off on Linux.
pub fn stdio() -> (tokio::io::Stdin, tokio::io::Stdout) {
    (tokio::io::stdin(), tokio::io::stdout())
}

/// # Non-blocking StdIO Transport
///
/// Like [`stdio`], but drives the standard streams through Tokio's
/// non-blocking pipe driver when both are FIFOs — the way MCP clients launch
/// stdio servers — so reads and writes run on the IO reactor instead of the
/// blocking thread pool. This avoids a cross-thread hand-off per message under
/// concurrent requests.
///
/// On Linux the pipe descriptors are reopened through `/proc/self/fd`, which
/// gives them their own open file descriptions: the process's own standard
/// streams, and any child process that inherits them, keep their blocking
/// flags. Every other platform (macOS, Windows) and any stream that is not a
/// FIFO falls back to the blocking [`stdio`] handles, so this can be used
/// unconditionally.
///
/// # Panics
///
/// On Linux, panics if called outside of a Tokio runtime with IO enabled,
/// mirroring `tokio::net::unix::pipe::Receiver::from_owned_fd`.
pub fn stdio_pipes() -> (StdioReader, StdioWriter) {
    #[cfg(target_os = "linux")]
    if let Some(pair) = try_pipe_transport() {
        return pair;
    }
    (
        StdioReader::Blocking(tokio::io::stdin()),
        StdioWriter::Blocking(tokio::io::stdout()),
    )
}

/// Takes the pipe fast path only when both standard streams are FIFOs.
///
/// The descriptors are reopened through `/proc/self/fd` rather than
/// duplicated. A duplicate shares its open file description with the original,
/// and `/proc/self/fd` opens a fresh one, so `O_NONBLOCK` stays private to the
/// transport instead of leaking into the process's standard streams (and into
/// children spawned with the inherited handles).
#[cfg(target_os = "linux")]
fn try_pipe_transport() -> Option<(StdioReader, StdioWriter)> {
    use std::os::fd::AsFd;

    let stdin = std::io::stdin().as_fd().try_clone_to_owned().ok()?;
    if !is_fifo(&stdin) {
        return None;
    }
    let stdout = std::io::stdout().as_fd().try_clone_to_owned().ok()?;
    if !is_fifo(&stdout) {
        return None;
    }

    let read = reopen_nonblocking("/proc/self/fd/0", false)?;
    let write = reopen_nonblocking("/proc/self/fd/1", true)?;
    pipe_transport(read, write)
}

/// Reopens `path` with `O_NONBLOCK`, on its own open file description.
#[cfg(target_os = "linux")]
fn reopen_nonblocking(path: &str, write: bool) -> Option<std::os::fd::OwnedFd> {
    use std::os::unix::fs::OpenOptionsExt;

    let mut options = std::fs::OpenOptions::new();
    options.read(!write).write(write);
    options.custom_flags(libc::O_NONBLOCK);
    options.open(path).ok().map(Into::into)
}

/// Wraps two FIFO descriptors as a non-blocking pipe transport.
#[cfg(target_os = "linux")]
fn pipe_transport(
    read: std::os::fd::OwnedFd,
    write: std::os::fd::OwnedFd,
) -> Option<(StdioReader, StdioWriter)> {
    let read = tokio::net::unix::pipe::Receiver::from_owned_fd(read).ok()?;
    let write = tokio::net::unix::pipe::Sender::from_owned_fd(write).ok()?;
    Some((StdioReader::Pipe(read), StdioWriter::Pipe(write)))
}

#[cfg(target_os = "linux")]
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
    /// Non-blocking standard input backed by a Linux pipe.
    #[cfg(target_os = "linux")]
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
            #[cfg(target_os = "linux")]
            Self::Pipe(reader) => Pin::new(reader).poll_read(cx, buf),
        }
    }
}

/// Writer half of the transport returned by [`stdio_pipes`].
#[non_exhaustive]
pub enum StdioWriter {
    /// Blocking standard output, as returned by [`stdio`].
    Blocking(tokio::io::Stdout),
    /// Non-blocking standard output backed by a Linux pipe.
    #[cfg(target_os = "linux")]
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
            #[cfg(target_os = "linux")]
            Self::Pipe(writer) => Pin::new(writer).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Blocking(writer) => Pin::new(writer).poll_flush(cx),
            #[cfg(target_os = "linux")]
            Self::Pipe(writer) => Pin::new(writer).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Blocking(writer) => Pin::new(writer).poll_shutdown(cx),
            #[cfg(target_os = "linux")]
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
            #[cfg(target_os = "linux")]
            Self::Pipe(writer) => Pin::new(writer).poll_write_vectored(cx, bufs),
        }
    }

    fn is_write_vectored(&self) -> bool {
        match self {
            Self::Blocking(writer) => writer.is_write_vectored(),
            #[cfg(target_os = "linux")]
            Self::Pipe(writer) => writer.is_write_vectored(),
        }
    }
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "linux")]
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

    #[cfg(target_os = "linux")]
    fn open_flags(fd: std::os::fd::RawFd) -> i32 {
        // SAFETY: every caller passes a live descriptor and F_GETFL only reads.
        unsafe { libc::fcntl(fd, libc::F_GETFL) }
    }

    #[cfg(target_os = "linux")]
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

    /// The reopen exists to keep `O_NONBLOCK` off the process's own standard
    /// streams, so pin the property on a FIFO: the reopened descriptor is
    /// non-blocking and the descriptor it came from stays blocking.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn reopening_a_fifo_keeps_the_original_flags() {
        use std::os::fd::AsRawFd;

        use super::{is_fifo, reopen_nonblocking};

        let path = std::env::temp_dir().join(format!("rmcp-fifo-reopen-{}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&path)
                .status()
                .expect("failed to run `mkfifo`")
                .success()
        );

        // O_RDWR on a FIFO does not wait for a peer, so the reopen cannot block.
        let original = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .expect("failed to open the fifo");
        assert_eq!(open_flags(original.as_raw_fd()) & libc::O_NONBLOCK, 0);

        let reopened =
            reopen_nonblocking(&format!("/proc/self/fd/{}", original.as_raw_fd()), false)
                .expect("failed to reopen the fifo");
        assert!(is_fifo(&reopened));
        assert_ne!(open_flags(reopened.as_raw_fd()) & libc::O_NONBLOCK, 0);
        assert_eq!(open_flags(original.as_raw_fd()) & libc::O_NONBLOCK, 0);

        drop(reopened);
        drop(original);
        let _ = std::fs::remove_file(&path);
    }

    /// The check the reviewer asked for: descriptor 0 keeps its flags.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn stdio_pipes_leaves_descriptor_zero_flags_alone() {
        use std::os::fd::AsRawFd;

        use super::stdio_pipes;

        let before = open_flags(std::io::stdin().as_raw_fd());
        let _transport = stdio_pipes();
        let after = open_flags(std::io::stdin().as_raw_fd());
        assert_eq!(before, after);
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn fifo_detection_rejects_character_devices() {
        use super::is_fifo;

        let dev_null: std::os::fd::OwnedFd = std::fs::File::open("/dev/null")
            .expect("failed to open /dev/null")
            .into();
        assert!(!is_fifo(&dev_null));
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
