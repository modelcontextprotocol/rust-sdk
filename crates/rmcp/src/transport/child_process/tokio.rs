use tokio_util::compat::{Compat, TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

use crate::transport::child_process::{
    builder::CommandConfig,
    runner::{ChildProcessInstance, ChildProcessRunner, RunnerSpawnError},
};

pub struct TokioChildProcessRunner {}

/// An implementation for the tokio Child Process
pub struct TokioChildProcess {
    inner: tokio::process::Child,
    /// The PID at the time of spawning.
    pid: u32,
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
        self.pid
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
            .map_err(RunnerSpawnError::SpawnError)
            .and_then(|child| {
                let pid = child.id().ok_or_else(|| RunnerSpawnError::NoPidAssigned)?;
                Ok(TokioChildProcess { inner: child, pid })
            })
    }
}

#[cfg(test)]
mod test {

    use crate::transport::CommandBuilder;
    use tokio::process::Command;

    use super::*;

    async fn check_pid(pid: u32) -> std::io::Result<bool> {
        // This command will output only process numbers on each line.
        let output = Command::new("ps")
            .arg("-o")
            .arg("pid=")
            .arg("-p")
            .arg(pid.to_string())
            .output()
            .await?;

        let output_str = String::from_utf8_lossy(&output.stdout);
        Ok(output_str
            .lines()
            .any(|line| line.trim() == pid.to_string()))
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_kill_on_drop() {
        let child = CommandBuilder::<TokioChildProcessRunner>::new("sleep")
            .args(["10"])
            .spawn_raw()
            .expect("Failed to spawn child process");

        let pid = child.pid();

        // Drop the child process without waiting for it to exit, which should kill it due to `kill_on_drop(true)`.
        drop(child);

        // Wait a moment to ensure the process has been killed.
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        let pid_found = check_pid(pid).await.expect("Failed to check if PID exists");

        assert!(!pid_found, "Child process was not killed on drop");
    }

    #[tokio::test]
    async fn test_graceful_shutdown() {
        let mut child = CommandBuilder::<TokioChildProcessRunner>::new("sleep")
            .args(["10"])
            .spawn_raw()
            .expect("Failed to spawn child process");

        let pid = child.pid();

        // Sleep a moment to ensure the process is running before we attempt to shut it down.
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        child
            .graceful_shutdown()
            .await
            .expect("Failed to gracefully shutdown child process");

        // We should not need to wait here since we await the graceful shutdown above.
        // Graceful shutdown *should* cover waiting for the process to exit.
        let pid_found = check_pid(pid).await.expect("Failed to check if PID exists");
        assert!(!pid_found, "Child process was not shutdown");
    }
}
