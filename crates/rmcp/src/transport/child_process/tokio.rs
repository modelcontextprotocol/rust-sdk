use tokio_util::compat::{Compat, TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

use crate::transport::child_process::{
    builder::CommandConfig,
    runner::{ChildProcessInstance, ChildProcessRunner, RunnerSpawnError},
};

pub struct TokioChildProcessRunner {}

/// An implementation for the tokio Child Process
pub struct TokioChildProcess {
    inner: tokio::process::Child,
}

impl ChildProcessInstance for TokioChildProcess {
    type Stdin = Compat<tokio::process::ChildStdin>;

    type Stdout = Compat<tokio::process::ChildStdout>;

    type Stderr = Compat<tokio::process::ChildStderr>;

    fn take_stdin(&mut self) -> super::runner::StreamSlot<Self::Stdin> {
        match self.inner.stdin.take() {
            Some(stdin) => super::runner::StreamSlot::Available(stdin.compat_write()),
            None => super::runner::StreamSlot::Unused,
        }
    }

    fn take_stdout(&mut self) -> super::runner::StreamSlot<Self::Stdout> {
        match self.inner.stdout.take() {
            Some(stdout) => super::runner::StreamSlot::Available(stdout.compat()),
            None => super::runner::StreamSlot::Unused,
        }
    }

    fn take_stderr(&mut self) -> super::runner::StreamSlot<Self::Stderr> {
        match self.inner.stderr.take() {
            Some(stderr) => super::runner::StreamSlot::Available(stderr.compat()),
            None => super::runner::StreamSlot::Unused,
        }
    }

    fn pid(&self) -> u32 {
        // TODO: Consider refactor to return Option<u32> to avoid confusion of 0 as a valid PID.
        self.inner.id().unwrap_or(0)
    }

    fn wait<'s>(
        &'s mut self,
    ) -> impl Future<Output = std::io::Result<std::process::ExitStatus>> + Send + 's {
        self.inner.wait()
    }

    fn graceful_shutdown<'s>(
        &'s mut self,
    ) -> impl Future<Output = std::io::Result<()>> + Send + 's {
        // TODO: Implement graceful shutdown on unix with SIGTERM. And look into graceful shutdown on windows as well.
        self.inner.kill()
    }

    fn kill<'s>(&'s mut self) -> impl Future<Output = std::io::Result<()>> + Send + 's {
        self.inner.kill()
    }
}

impl ChildProcessRunner for TokioChildProcessRunner {
    type Instance = TokioChildProcess;
    fn spawn(command_config: CommandConfig) -> Result<Self::Instance, RunnerSpawnError> {
        tokio::process::Command::new(command_config.command)
            .args(command_config.args)
            .envs(command_config.env)
            .stdin(command_config.stdio_config.stdin)
            .stdout(command_config.stdio_config.stdout)
            .stderr(command_config.stdio_config.stderr)
            .current_dir(
                command_config
                    .cwd
                    .unwrap_or_else(|| std::env::current_dir().unwrap()),
            )
            .kill_on_drop(true)
            .spawn()
            .map(|child| TokioChildProcess { inner: child })
            .map_err(RunnerSpawnError::SpawnError)
    }
}
