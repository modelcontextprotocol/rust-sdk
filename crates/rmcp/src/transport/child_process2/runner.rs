use futures::{
    FutureExt,
    io::{AsyncRead, AsyncWrite},
};

use crate::{transport::child_process2::builder::CommandConfig, util::PinnedFuture};

/// A simple enum for describing if a stream is available, unused, or already taken.
#[derive(Debug)]
pub enum StreamSlot<S> {
    /// The stream is not used in this implementation.
    Unused,
    /// The stream is available for use, and can be taken.
    Available(S),
    /// The stream has already been taken, and is no longer available.
    Taken,
}

impl<S> From<StreamSlot<S>> for Option<S> {
    fn from(slot: StreamSlot<S>) -> Self {
        match slot {
            StreamSlot::Unused => None,
            StreamSlot::Available(s) => Some(s),
            StreamSlot::Taken => None,
        }
    }
}

/// The contract for what an instance of a child process
/// must provide to be used with a transport.
pub trait ChildProcessInstance {
    /// The input stream for the command
    type Stdin: AsyncWrite + Unpin + Send;

    /// The output stream of the command
    type Stdout: AsyncRead + Unpin + Send;

    /// The error stream of the command
    type Stderr: AsyncRead + Unpin + Send;

    fn take_stdin(&mut self) -> StreamSlot<Self::Stdin>;
    fn take_stdout(&mut self) -> StreamSlot<Self::Stdout>;
    fn take_stderr(&mut self) -> StreamSlot<Self::Stderr>;

    fn pid(&self) -> u32;
    fn wait<'s>(
        &'s mut self,
    ) -> impl Future<Output = std::io::Result<std::process::ExitStatus>> + Send + 's;
    fn graceful_shutdown<'s>(&'s mut self)
    -> impl Future<Output = std::io::Result<()>> + Send + 's;
    fn kill<'s>(&'s mut self) -> impl Future<Output = std::io::Result<()>> + Send + 's;
}

/// A subset of functionality of [ChildProcessInstance] that only includes the
/// functions used to control or wait for the process.
pub trait ChildProcessControl {
    fn pid(&self) -> u32;
    fn wait<'s>(&'s mut self) -> PinnedFuture<'s, std::io::Result<std::process::ExitStatus>>;
    fn graceful_shutdown<'s>(&'s mut self) -> PinnedFuture<'s, std::io::Result<()>>;
    fn kill<'s>(&'s mut self) -> PinnedFuture<'s, std::io::Result<()>>;
}

/// Auto-implement ChildProcessControl for any ChildProcessInstance, since it has all the required methods.
impl<T> ChildProcessControl for T
where
    T: ChildProcessInstance,
{
    fn pid(&self) -> u32 {
        ChildProcessInstance::pid(self)
    }

    fn wait<'s>(&'s mut self) -> PinnedFuture<'s, std::io::Result<std::process::ExitStatus>> {
        ChildProcessInstance::wait(self).boxed()
    }

    fn graceful_shutdown<'s>(&'s mut self) -> PinnedFuture<'s, std::io::Result<()>> {
        ChildProcessInstance::graceful_shutdown(self).boxed()
    }

    fn kill<'s>(&'s mut self) -> PinnedFuture<'s, std::io::Result<()>> {
        ChildProcessInstance::kill(self).boxed()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RunnerSpawnError {
    /// The child process instance failed to spawn.
    #[error("Failed to spawn child process: {0}")]
    SpawnError(#[from] std::io::Error),
    #[error("Other error: {0}")]
    Other(Box<dyn std::error::Error + Send + Sync>),
}

pub trait ChildProcessRunner {
    /// The implementation of the child process instance that this runner will spawn.
    type Instance: ChildProcessInstance;

    fn spawn(command_config: CommandConfig) -> Result<Self::Instance, RunnerSpawnError>;
}

/// A containing wrapper around a child process instance. This struct erases the type
/// by extracting some parts of the [ChildProcessInstance] trait into a common struct,
/// and then only exposes the control methods.
pub struct ChildProcess {
    stdin: StreamSlot<Box<dyn AsyncWrite + Unpin + Send>>,
    stdout: StreamSlot<Box<dyn AsyncRead + Unpin + Send>>,
    stderr: StreamSlot<Box<dyn AsyncRead + Unpin + Send>>,
    inner: Box<dyn ChildProcessControl + Send>,
}

impl ChildProcess {
    pub fn new<C>(mut instance: C) -> Self
    where
        C: ChildProcessInstance + Send + 'static,
    {
        Self {
            stdin: match instance.take_stdin() {
                StreamSlot::Available(s) => StreamSlot::Available(Box::new(s)),
                StreamSlot::Unused => StreamSlot::Unused,
                StreamSlot::Taken => {
                    panic!("Stdin stream was already taken during ChildProcess construction")
                }
            },
            stdout: match instance.take_stdout() {
                StreamSlot::Available(s) => StreamSlot::Available(Box::new(s)),
                StreamSlot::Unused => StreamSlot::Unused,
                StreamSlot::Taken => {
                    panic!("Stdout stream was already taken during ChildProcess construction")
                }
            },
            stderr: match instance.take_stderr() {
                StreamSlot::Available(s) => StreamSlot::Available(Box::new(s)),
                StreamSlot::Unused => StreamSlot::Unused,
                StreamSlot::Taken => {
                    panic!("Stderr stream was already taken during ChildProcess construction")
                }
            },
            inner: Box::new(instance),
        }
    }

    pub fn split(
        self,
    ) -> (
        Option<Box<dyn AsyncRead + Unpin + Send>>,
        Option<Box<dyn AsyncWrite + Unpin + Send>>,
        Option<Box<dyn AsyncRead + Unpin + Send>>,
        Box<dyn ChildProcessControl + Send>,
    ) {
        (
            self.stdout.into(),
            self.stdin.into(),
            self.stderr.into(),
            self.inner,
        )
    }
}

impl ChildProcessInstance for ChildProcess {
    type Stdin = Box<dyn AsyncWrite + Unpin + Send>;

    type Stdout = Box<dyn AsyncRead + Unpin + Send>;

    type Stderr = Box<dyn AsyncRead + Unpin + Send>;

    fn take_stdin(&mut self) -> StreamSlot<Self::Stdin> {
        match self.stdin {
            StreamSlot::Available(_) => std::mem::replace(&mut self.stdin, StreamSlot::Taken),
            StreamSlot::Unused => StreamSlot::Unused,
            StreamSlot::Taken => StreamSlot::Taken,
        }
    }

    fn take_stdout(&mut self) -> StreamSlot<Self::Stdout> {
        match self.stdout {
            StreamSlot::Available(_) => std::mem::replace(&mut self.stdout, StreamSlot::Taken),
            StreamSlot::Unused => StreamSlot::Unused,
            StreamSlot::Taken => StreamSlot::Taken,
        }
    }

    fn take_stderr(&mut self) -> StreamSlot<Self::Stderr> {
        match self.stderr {
            StreamSlot::Available(_) => std::mem::replace(&mut self.stderr, StreamSlot::Taken),
            StreamSlot::Unused => StreamSlot::Unused,
            StreamSlot::Taken => StreamSlot::Taken,
        }
    }

    fn pid(&self) -> u32 {
        self.inner.pid()
    }

    fn wait<'s>(
        &'s mut self,
    ) -> impl Future<Output = std::io::Result<std::process::ExitStatus>> + Send + 's {
        self.inner.wait()
    }

    fn graceful_shutdown<'s>(
        &'s mut self,
    ) -> impl Future<Output = std::io::Result<()>> + Send + 's {
        self.inner.graceful_shutdown()
    }

    fn kill<'s>(&'s mut self) -> impl Future<Output = std::io::Result<()>> + Send + 's {
        self.inner.kill()
    }
}
