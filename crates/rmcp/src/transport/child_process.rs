use std::process::Stdio;

use process_wrap::tokio::{TokioChildWrapper, TokioCommandWrap};
use tokio::{
    io::AsyncRead,
    process::{ChildStderr, ChildStdin, ChildStdout},
};

use super::{IntoTransport, Transport};
use crate::service::ServiceRole;

/// The parts of a child process.
type ChildProcessParts = (
    Box<dyn TokioChildWrapper>,
    ChildStdout,
    ChildStdin,
    Option<ChildStderr>,
);

/// Extract the stdio handles from a spawned child.
/// Returns `(child, stdout, stdin, stderr)` where `stderr` is `Some` only
/// if the process was spawned with `Stdio::piped()`.
#[inline]
fn child_process(mut child: Box<dyn TokioChildWrapper>) -> std::io::Result<ChildProcessParts> {
    let child_stdin = match child.inner_mut().stdin().take() {
        Some(stdin) => stdin,
        None => return Err(std::io::Error::other("stdin was already taken")),
    };
    let child_stdout = match child.inner_mut().stdout().take() {
        Some(stdout) => stdout,
        None => return Err(std::io::Error::other("stdout was already taken")),
    };
    let child_stderr = child.inner_mut().stderr().take();
    Ok((child, child_stdout, child_stdin, child_stderr))
}

pub struct TokioChildProcess {
    child: ChildWithCleanup,
    child_stdin: ChildStdin,
    child_stdout: ChildStdout,
}

pub struct ChildWithCleanup {
    inner: Box<dyn TokioChildWrapper>,
}

impl Drop for ChildWithCleanup {
    fn drop(&mut self) {
        if let Err(e) = self.inner.start_kill() {
            tracing::warn!("Failed to kill child process: {e}");
        }
    }
}

// we hold the child process with stdout, for it's easier to implement AsyncRead
pin_project_lite::pin_project! {
    pub struct TokioChildProcessOut {
        child: ChildWithCleanup,
        #[pin]
        child_stdout: ChildStdout,
    }
}

impl TokioChildProcessOut {
    /// Get the process ID of the child process.
    pub fn id(&self) -> Option<u32> {
        self.child.inner.id()
    }
}

impl AsyncRead for TokioChildProcessOut {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        self.project().child_stdout.poll_read(cx, buf)
    }
}

impl TokioChildProcess {
    /// Convenience: spawn with default `piped` stdio
    pub fn new(command: impl Into<TokioCommandWrap>) -> std::io::Result<Self> {
        let (proc, _ignored) = TokioChildProcessBuilder::new(command).spawn()?;
        Ok(proc)
    }

    /// Builder entry-point allowing fine-grained stdio control.
    pub fn builder(command: impl Into<TokioCommandWrap>) -> TokioChildProcessBuilder {
        TokioChildProcessBuilder::new(command)
    }

    /// Get the process ID of the child process.
    pub fn id(&self) -> Option<u32> {
        self.child.inner.id()
    }

    /// Split this helper into a reader (stdout) and writer (stdin).
    pub fn split(self) -> (TokioChildProcessOut, ChildStdin) {
        let TokioChildProcess {
            child,
            child_stdin,
            child_stdout,
        } = self;
        (
            TokioChildProcessOut {
                child,
                child_stdout,
            },
            child_stdin,
        )
    }
}

/// Builder for `TokioChildProcess` allowing custom `Stdio` configuration.
pub struct TokioChildProcessBuilder {
    cmd: TokioCommandWrap,
    stdin: Stdio,
    stdout: Stdio,
    stderr: Stdio,
}

impl TokioChildProcessBuilder {
    fn new(cmd: impl Into<TokioCommandWrap>) -> Self {
        Self {
            cmd: cmd.into(),
            stdin: Stdio::piped(),
            stdout: Stdio::piped(),
            stderr: Stdio::inherit(),
        }
    }

    /// Override the child stdin configuration.
    pub fn stdin(mut self, io: impl Into<Stdio>) -> Self {
        self.stdin = io.into();
        self
    }
    /// Override the child stdout configuration.
    pub fn stdout(mut self, io: impl Into<Stdio>) -> Self {
        self.stdout = io.into();
        self
    }
    /// Override the child stderr configuration.
    pub fn stderr(mut self, io: impl Into<Stdio>) -> Self {
        self.stderr = io.into();
        self
    }

    /// Spawn the child process. Returns the transport plus an optional captured stderr handle.
    pub fn spawn(mut self) -> std::io::Result<(TokioChildProcess, Option<ChildStderr>)> {
        self.cmd
            .command_mut()
            .stdin(self.stdin)
            .stdout(self.stdout)
            .stderr(self.stderr);

        let (child, stdout, stdin, stderr_opt) = child_process(self.cmd.spawn()?)?;

        let proc = TokioChildProcess {
            child: ChildWithCleanup { inner: child },
            child_stdin: stdin,
            child_stdout: stdout,
        };
        Ok((proc, stderr_opt))
    }
}

impl<R: ServiceRole> IntoTransport<R, std::io::Error, ()> for TokioChildProcess {
    fn into_transport(self) -> impl Transport<R, Error = std::io::Error> + 'static {
        IntoTransport::<R, std::io::Error, super::async_rw::TransportAdapterAsyncRW>::into_transport(
            self.split(),
        )
    }
}

pub trait ConfigureCommandExt {
    fn configure(self, f: impl FnOnce(&mut Self)) -> Self;
}

impl ConfigureCommandExt for tokio::process::Command {
    fn configure(mut self, f: impl FnOnce(&mut Self)) -> Self {
        f(&mut self);
        self
    }
}
