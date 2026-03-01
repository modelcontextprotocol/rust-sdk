use std::{collections::HashMap, path::PathBuf, process::Stdio};

use crate::transport::{ChildProcess, ChildProcessRunner, child_process::runner::RunnerSpawnError};

/// A builder for constructing a command to spawn a child process, with typical command
/// configuration options like `args` and `current_dir`.
pub struct CommandBuilder<R> {
    config: CommandConfig,
    _marker: std::marker::PhantomData<R>,
}

#[derive(Debug, thiserror::Error)]
pub enum CommandBuilderError {
    #[error("Command cannot be empty")]
    EmptyCommand,
}

impl<R> CommandBuilder<R> {
    /// Create a CommandBuilder from an argv-style list of strings, where the first element is the command, and the rest are the args.
    pub fn from_argv<I, S>(argv: I) -> Result<Self, CommandBuilderError>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut iter = argv.into_iter();

        // Pop the first element as the command, and use the rest as args
        let command = match iter.next() {
            Some(cmd) => cmd.into(),
            None => return Err(CommandBuilderError::EmptyCommand),
        };

        let args = iter.map(|s| s.into()).collect();
        Ok(Self {
            config: CommandConfig {
                command,
                args,
                ..Default::default()
            },
            _marker: std::marker::PhantomData,
        })
    }

    /// Create a CommandBuilder from a command and an optional list of args.
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            config: CommandConfig {
                command: command.into(),
                ..Default::default()
            },
            _marker: std::marker::PhantomData,
        }
    }

    /// Add a single argument to the command.
    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.config.args.push(arg.into());
        self
    }

    /// Add multiple arguments to the command.
    pub fn args(mut self, args: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.config
            .args
            .extend(args.into_iter().map(|arg| arg.into()));
        self
    }

    /// Set an environment variable for the command.
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.config.env.insert(key.into(), value.into());
        self
    }

    /// Set multiple environment variables for the command.
    pub fn envs(
        mut self,
        envs: impl IntoIterator<Item = (impl Into<String>, impl Into<String>)>,
    ) -> Self {
        self.config
            .env
            .extend(envs.into_iter().map(|(k, v)| (k.into(), v.into())));
        self
    }

    /// Sets what happens to stderr for the command.
    /// By default if not set, stderr is inherited from the parent process.
    pub fn stderr(mut self, _stdio: Stdio) -> Self {
        self.config.stdio_config.stderr = _stdio;
        self
    }

    pub fn current_dir(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.config.cwd = Some(cwd.into());
        self
    }
}

/// A structure that requests how the child process streams should
/// be configured when spawning.
#[derive(Debug)]
pub struct StdioConfig {
    pub stdin: Stdio,
    pub stdout: Stdio,
    pub stderr: Stdio,
}

impl Default for StdioConfig {
    fn default() -> Self {
        Self {
            stdin: Stdio::piped(),
            stdout: Stdio::piped(),
            stderr: Stdio::inherit(),
        }
    }
}

/// A structure that requests how the command should be executed
#[derive(Debug, Default)]
pub struct CommandConfig {
    pub command: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub stdio_config: StdioConfig,
    pub env: HashMap<String, String>,
}

impl<R> CommandBuilder<R>
where
    R: ChildProcessRunner,
{
    /// Spawn the command into its typed child process instance type.
    pub fn spawn_raw(self) -> Result<R::Instance, RunnerSpawnError> {
        R::spawn(self.config)
    }

    /// Spawn a child process struct that erases the specific child process instance type, and only exposes the control methods.
    ///
    /// Requires `R::Instance` to be [Send] and `'static`.
    pub fn spawn_dyn(self) -> Result<ChildProcess, RunnerSpawnError>
    where
        R::Instance: Send + 'static,
    {
        let instance = self.spawn_raw()?;
        Ok(ChildProcess::new(instance))
    }
}
