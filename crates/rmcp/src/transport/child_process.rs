use std::{ffi::OsString, path::PathBuf, process::Stdio};

use futures::future::Future;
use process_wrap::tokio::{ChildWrapper, CommandWrap};
use tokio::{
    io::AsyncRead,
    process::{ChildStderr, ChildStdin, ChildStdout},
};

use super::{RxJsonRpcMessage, Transport, TxJsonRpcMessage, async_rw::AsyncRwTransport};
use crate::RoleClient;

const MAX_WAIT_ON_DROP_SECS: u64 = 3;
/// The parts of a child process.
type ChildProcessParts = (
    Box<dyn ChildWrapper>,
    ChildStdout,
    ChildStdin,
    Option<ChildStderr>,
);

/// Extract the stdio handles from a spawned child.
/// Returns `(child, stdout, stdin, stderr)` where `stderr` is `Some` only
/// if the process was spawned with `Stdio::piped()`.
#[inline]
fn child_process(mut child: Box<dyn ChildWrapper>) -> std::io::Result<ChildProcessParts> {
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

// ---------------------------------------------------------------------------
// SEP-1024: Command approval handler
// ---------------------------------------------------------------------------

/// Information about a command that is about to be executed.
///
/// This is presented to a [`CommandApprovalHandler`] so the user (or policy)
/// can inspect exactly what will be spawned before it happens.
///
/// See [SEP-1024](https://modelcontextprotocol.io/community/seps/1024-mcp-client-security-requirements-for-local-server-.md)
/// for the security rationale.
#[derive(Debug, Clone)]
pub struct CommandInfo {
    /// The program that will be executed (e.g. `"npx"`, `"uvx"`).
    pub program: OsString,
    /// The arguments that will be passed to the program.
    pub args: Vec<OsString>,
    /// Environment variables that will be set (or cleared) for the child.
    /// Each entry is `(key, Option<value>)` – `None` means the variable is
    /// explicitly removed.
    pub envs: Vec<(OsString, Option<OsString>)>,
    /// The working directory for the child, if explicitly set.
    pub working_dir: Option<PathBuf>,
}

impl CommandInfo {
    /// Build a `CommandInfo` by inspecting a [`CommandWrap`].
    fn from_command_wrap(cmd: &CommandWrap) -> Self {
        let std_cmd = cmd.command().as_std();
        Self {
            program: std_cmd.get_program().to_owned(),
            args: std_cmd.get_args().map(|a| a.to_owned()).collect(),
            envs: std_cmd
                .get_envs()
                .map(|(k, v)| (k.to_owned(), v.map(|v| v.to_owned())))
                .collect(),
            working_dir: std_cmd.get_current_dir().map(|p| p.to_owned()),
        }
    }

    /// Return a human-readable representation of the full command line,
    /// suitable for display in a consent dialog.
    pub fn command_line_display(&self) -> String {
        let mut parts = Vec::with_capacity(1 + self.args.len());
        parts.push(self.program.to_string_lossy().into_owned());
        for arg in &self.args {
            let s = arg.to_string_lossy().into_owned();
            if s.contains(' ') || s.contains('"') || s.is_empty() {
                parts.push(format!("\"{}\"", s.replace('"', "\\\"")));
            } else {
                parts.push(s);
            }
        }
        parts.join(" ")
    }
}

impl std::fmt::Display for CommandInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.command_line_display())?;
        if let Some(dir) = &self.working_dir {
            write!(f, " (in {})", dir.display())?;
        }
        Ok(())
    }
}

/// A handler that decides whether a command is allowed to execute.
///
/// Implementations of this trait are called by [`TokioChildProcessBuilder::spawn`]
/// **before** the child process is created, giving the caller a chance to
/// inspect the full command line and either approve or reject it.
///
/// # SEP-1024
///
/// [SEP-1024](https://modelcontextprotocol.io/community/seps/1024-mcp-client-security-requirements-for-local-server-.md)
/// requires MCP clients to present a consent dialog before executing local
/// server installation commands. This trait is the integration point for that
/// requirement.
///
/// # Example
///
/// ```rust,no_run
/// use rmcp::transport::child_process::{CommandApprovalHandler, CommandInfo};
///
/// struct MyPolicyHandler;
///
/// impl CommandApprovalHandler for MyPolicyHandler {
///     fn approve(
///         &self,
///         info: &CommandInfo,
///     ) -> futures::future::BoxFuture<'_, std::io::Result<bool>> {
///         // Clone any data needed inside the future.
///         let display = info.to_string();
///         Box::pin(async move {
///             // Check against an allowlist, prompt the user, etc.
///             println!("Allow `{display}`?");
///             Ok(true)
///         })
///     }
/// }
/// ```
pub trait CommandApprovalHandler: Send + Sync {
    /// Inspect the command described by `info` and return `Ok(true)` to allow
    /// execution, `Ok(false)` to deny it, or `Err` on failure.
    fn approve(&self, info: &CommandInfo) -> futures::future::BoxFuture<'_, std::io::Result<bool>>;
}

/// An approval handler that always approves execution without prompting.
///
/// This is appropriate for trusted/testing contexts where no user interaction
/// is desired. **Do not** use this in user-facing applications that install
/// servers from untrusted sources.
#[derive(Debug, Clone, Copy, Default)]
pub struct AlwaysApproveHandler;

impl CommandApprovalHandler for AlwaysApproveHandler {
    fn approve(
        &self,
        _info: &CommandInfo,
    ) -> futures::future::BoxFuture<'_, std::io::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
}

/// An approval handler that always denies execution.
///
/// Useful for testing or for contexts where spawning child processes should
/// be unconditionally blocked.
#[derive(Debug, Clone, Copy, Default)]
pub struct AlwaysDenyHandler;

impl CommandApprovalHandler for AlwaysDenyHandler {
    fn approve(
        &self,
        _info: &CommandInfo,
    ) -> futures::future::BoxFuture<'_, std::io::Result<bool>> {
        Box::pin(async { Ok(false) })
    }
}

/// An approval handler that prompts the user on the standard I/O console.
///
/// This is the reference implementation of the consent dialog required by
/// [SEP-1024](https://modelcontextprotocol.io/community/seps/1024-mcp-client-security-requirements-for-local-server-.md).
///
/// The handler prints the full command line and waits for the user to type
/// `y` or `n`. Any other input (including EOF) is treated as denial.
///
/// # Example
///
/// ```rust,no_run
/// use rmcp::transport::child_process::{StdioApprovalHandler, TokioChildProcess};
/// use tokio::process::Command;
///
/// # async fn example() -> std::io::Result<()> {
/// let (proc, _stderr) = TokioChildProcess::builder(Command::new("npx"))
///     .approval_handler(StdioApprovalHandler)
///     .spawn()
///     .await?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, Copy, Default)]
pub struct StdioApprovalHandler;

impl CommandApprovalHandler for StdioApprovalHandler {
    fn approve(&self, info: &CommandInfo) -> futures::future::BoxFuture<'_, std::io::Result<bool>> {
        let command_line = info.command_line_display();
        let working_dir = info.working_dir.clone();
        Box::pin(async move {
            // Use blocking I/O via spawn_blocking so we don't block the
            // async runtime.
            tokio::task::spawn_blocking(move || {
                use std::io::Write;

                let mut stderr = std::io::stderr().lock();
                writeln!(stderr)?;
                writeln!(
                    stderr,
                    "⚠️  An MCP server wants to execute the following command:"
                )?;
                writeln!(stderr)?;
                writeln!(stderr, "    {command_line}")?;
                if let Some(dir) = &working_dir {
                    writeln!(stderr, "    (in directory: {})", dir.display())?;
                }
                writeln!(stderr)?;
                writeln!(stderr, "  This will create a new process on your machine.")?;
                writeln!(
                    stderr,
                    "  Only approve if you trust the source of this command."
                )?;
                writeln!(stderr)?;
                write!(stderr, "Allow execution? [y/N] ")?;
                stderr.flush()?;

                let mut input = String::new();
                std::io::stdin().read_line(&mut input)?;
                let approved = matches!(input.trim(), "y" | "Y" | "yes" | "Yes" | "YES");
                if !approved {
                    writeln!(stderr, "  ✗ Command execution denied by user.")?;
                } else {
                    writeln!(stderr, "  ✓ Command execution approved.")?;
                }
                Ok(approved)
            })
            .await
            .map_err(|e| std::io::Error::other(format!("approval task failed: {e}")))?
        })
    }
}

/// Implement `CommandApprovalHandler` for closures / function pointers.
///
/// The closure receives an owned [`CommandInfo`] and returns a boxed future.
/// This avoids lifetime issues that arise when borrowing both `&self` and
/// `&CommandInfo` into the returned future.
///
/// ```rust,no_run
/// use rmcp::transport::child_process::{CommandApprovalHandler, CommandInfo};
///
/// let handler = |info: CommandInfo| -> futures::future::BoxFuture<'static, std::io::Result<bool>> {
///     Box::pin(async move {
///         println!("approve: {info}");
///         Ok(true)
///     })
/// };
/// ```
impl<F> CommandApprovalHandler for F
where
    F: Fn(CommandInfo) -> futures::future::BoxFuture<'static, std::io::Result<bool>> + Send + Sync,
{
    fn approve(&self, info: &CommandInfo) -> futures::future::BoxFuture<'_, std::io::Result<bool>> {
        (self)(info.clone())
    }
}

// ---------------------------------------------------------------------------
// TokioChildProcess
// ---------------------------------------------------------------------------

pub struct TokioChildProcess {
    child: ChildWithCleanup,
    transport: AsyncRwTransport<RoleClient, ChildStdout, ChildStdin>,
}

impl std::fmt::Debug for TokioChildProcess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokioChildProcess")
            .field("pid", &self.id())
            .finish()
    }
}

pub struct ChildWithCleanup {
    inner: Option<Box<dyn ChildWrapper>>,
}

impl Drop for ChildWithCleanup {
    fn drop(&mut self) {
        // We should not use start_kill(), instead we should use kill() to avoid zombies
        if let Some(mut inner) = self.inner.take() {
            // We don't care about the result, just try to kill it
            tokio::spawn(async move {
                if let Err(e) = Box::into_pin(inner.kill()).await {
                    tracing::warn!("Error killing child process: {}", e);
                }
            });
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
        self.child.inner.as_ref()?.id()
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
    /// Convenience: spawn with default `piped` stdio and **no** approval
    /// handler.
    ///
    /// This is appropriate when the caller has already validated the command
    /// or is constructing it from a trusted source. For user-facing
    /// applications, prefer [`TokioChildProcess::builder`] with an
    /// [`approval_handler`](TokioChildProcessBuilder::approval_handler).
    pub fn new(command: impl Into<CommandWrap>) -> std::io::Result<Self> {
        let (proc, _ignored) = TokioChildProcessBuilder::new(command).spawn_sync()?;
        Ok(proc)
    }

    /// Builder entry-point allowing fine-grained stdio control and an
    /// optional [`CommandApprovalHandler`].
    pub fn builder(command: impl Into<CommandWrap>) -> TokioChildProcessBuilder {
        TokioChildProcessBuilder::new(command)
    }

    /// Get the process ID of the child process.
    pub fn id(&self) -> Option<u32> {
        self.child.inner.as_ref()?.id()
    }

    /// Gracefully shutdown the child process
    ///
    /// This will first close the transport to the child process (the server),
    /// and wait for the child process to exit normally with a timeout.
    /// If the child process doesn't exit within the timeout, it will be killed.
    pub async fn graceful_shutdown(&mut self) -> std::io::Result<()> {
        if let Some(mut child) = self.child.inner.take() {
            self.transport.close().await?;

            let wait_fut = child.wait();
            tokio::select! {
                _ = tokio::time::sleep(std::time::Duration::from_secs(MAX_WAIT_ON_DROP_SECS)) => {
                    if let Err(e) = Box::into_pin(child.kill()).await {
                        tracing::warn!("Error killing child: {e}");
                        return Err(e);
                    }
                },
                res = wait_fut => {
                    match res {
                        Ok(status) => {
                            tracing::info!("Child exited gracefully {}", status);
                        }
                        Err(e) => {
                            tracing::warn!("Error waiting for child: {e}");
                            return Err(e);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Take ownership of the inner child process
    pub fn into_inner(mut self) -> Option<Box<dyn ChildWrapper>> {
        self.child.inner.take()
    }

    /// Split this helper into a reader (stdout) and writer (stdin).
    #[deprecated(
        since = "0.5.0",
        note = "use the Transport trait implementation instead"
    )]
    pub fn split(self) -> (TokioChildProcessOut, ChildStdin) {
        unimplemented!("This method is deprecated, use the Transport trait implementation instead");
    }
}

/// Builder for `TokioChildProcess` allowing custom `Stdio` configuration
/// and an optional [`CommandApprovalHandler`].
///
/// # SEP-1024 – Command Approval
///
/// When an [`approval_handler`](Self::approval_handler) is set, [`spawn`](Self::spawn)
/// will present the full command line to the handler **before** the child
/// process is created. If the handler returns `Ok(false)` the spawn is
/// aborted with an [`std::io::ErrorKind::PermissionDenied`] error.
///
/// ```rust,no_run
/// use rmcp::transport::child_process::{StdioApprovalHandler, TokioChildProcess};
/// use tokio::process::Command;
///
/// # async fn example() -> std::io::Result<()> {
/// let (proc, _stderr) = TokioChildProcess::builder(Command::new("npx"))
///     .approval_handler(StdioApprovalHandler)
///     .spawn()
///     .await?;
/// # Ok(())
/// # }
/// ```
pub struct TokioChildProcessBuilder {
    cmd: CommandWrap,
    stdin: Stdio,
    stdout: Stdio,
    stderr: Stdio,
    approval_handler: Option<Box<dyn CommandApprovalHandler>>,
}

impl TokioChildProcessBuilder {
    fn new(cmd: impl Into<CommandWrap>) -> Self {
        Self {
            cmd: cmd.into(),
            stdin: Stdio::piped(),
            stdout: Stdio::piped(),
            stderr: Stdio::inherit(),
            approval_handler: None,
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

    /// Set a [`CommandApprovalHandler`] that will be consulted before the
    /// child process is spawned.
    ///
    /// When set, [`spawn`](Self::spawn) becomes an `async` operation that
    /// calls the handler and only proceeds if it returns `Ok(true)`.
    pub fn approval_handler(mut self, handler: impl CommandApprovalHandler + 'static) -> Self {
        self.approval_handler = Some(Box::new(handler));
        self
    }

    /// Spawn the child process **after** consulting the approval handler (if
    /// any). Returns the transport plus an optional captured stderr handle.
    ///
    /// If the approval handler denies execution, this returns an
    /// [`std::io::Error`] with kind [`std::io::ErrorKind::PermissionDenied`].
    pub async fn spawn(mut self) -> std::io::Result<(TokioChildProcess, Option<ChildStderr>)> {
        // Configure stdio before extracting CommandInfo so the info reflects
        // the final state.
        self.cmd
            .command_mut()
            .stdin(self.stdin)
            .stdout(self.stdout)
            .stderr(self.stderr);

        // --- SEP-1024: approval gate ---
        if let Some(handler) = &self.approval_handler {
            let info = CommandInfo::from_command_wrap(&self.cmd);
            let approved = handler.approve(&info).await?;
            if !approved {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    format!("command execution denied: {info}"),
                ));
            }
        }

        let (child, stdout, stdin, stderr_opt) = child_process(self.cmd.spawn()?)?;

        let transport = AsyncRwTransport::new(stdout, stdin);
        let proc = TokioChildProcess {
            child: ChildWithCleanup { inner: Some(child) },
            transport,
        };
        Ok((proc, stderr_opt))
    }

    /// Spawn **synchronously** without consulting any approval handler.
    ///
    /// This is used internally by [`TokioChildProcess::new`] for backward
    /// compatibility. Prefer [`spawn`](Self::spawn) in new code.
    fn spawn_sync(mut self) -> std::io::Result<(TokioChildProcess, Option<ChildStderr>)> {
        self.cmd
            .command_mut()
            .stdin(self.stdin)
            .stdout(self.stdout)
            .stderr(self.stderr);

        let (child, stdout, stdin, stderr_opt) = child_process(self.cmd.spawn()?)?;

        let transport = AsyncRwTransport::new(stdout, stdin);
        let proc = TokioChildProcess {
            child: ChildWithCleanup { inner: Some(child) },
            transport,
        };
        Ok((proc, stderr_opt))
    }
}

impl Transport<RoleClient> for TokioChildProcess {
    type Error = std::io::Error;

    fn send(
        &mut self,
        item: TxJsonRpcMessage<RoleClient>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
        self.transport.send(item)
    }

    fn receive(&mut self) -> impl Future<Output = Option<RxJsonRpcMessage<RoleClient>>> + Send {
        self.transport.receive()
    }

    fn close(&mut self) -> impl Future<Output = Result<(), Self::Error>> + Send {
        self.graceful_shutdown()
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

#[cfg(unix)]
#[cfg(test)]
mod tests {
    use tokio::process::Command;

    use super::*;

    #[tokio::test]
    async fn test_tokio_child_process_drop() {
        let r = TokioChildProcess::new(Command::new("sleep").configure(|cmd| {
            cmd.arg("30");
        }));
        assert!(r.is_ok());
        let child_process = r.unwrap();
        let id = child_process.id();
        assert!(id.is_some());
        let id = id.unwrap();
        // Drop the child process
        drop(child_process);
        // Wait a moment to allow the cleanup task to run
        tokio::time::sleep(std::time::Duration::from_secs(MAX_WAIT_ON_DROP_SECS + 1)).await;
        // Check if the process is still running
        let status = Command::new("ps")
            .arg("-p")
            .arg(id.to_string())
            .status()
            .await;
        match status {
            Ok(status) => {
                assert!(
                    !status.success(),
                    "Process with PID {} is still running",
                    id
                );
            }
            Err(e) => {
                panic!("Failed to check process status: {}", e);
            }
        }
    }

    #[tokio::test]
    async fn test_tokio_child_process_graceful_shutdown() {
        let r = TokioChildProcess::new(Command::new("sleep").configure(|cmd| {
            cmd.arg("30");
        }));
        assert!(r.is_ok());
        let mut child_process = r.unwrap();
        let id = child_process.id();
        assert!(id.is_some());
        let id = id.unwrap();
        child_process.graceful_shutdown().await.unwrap();
        // Wait a moment to allow the cleanup task to run
        tokio::time::sleep(std::time::Duration::from_secs(MAX_WAIT_ON_DROP_SECS + 1)).await;
        // Check if the process is still running
        let status = Command::new("ps")
            .arg("-p")
            .arg(id.to_string())
            .status()
            .await;
        match status {
            Ok(status) => {
                assert!(
                    !status.success(),
                    "Process with PID {} is still running",
                    id
                );
            }
            Err(e) => {
                panic!("Failed to check process status: {}", e);
            }
        }
    }

    #[tokio::test]
    async fn test_approval_handler_approve() {
        let (proc, _stderr) = TokioChildProcess::builder(Command::new("echo").configure(|cmd| {
            cmd.arg("hello");
        }))
        .approval_handler(AlwaysApproveHandler)
        .spawn()
        .await
        .expect("spawn should succeed when approved");

        assert!(proc.id().is_some());
    }

    #[tokio::test]
    async fn test_approval_handler_deny() {
        let result = TokioChildProcess::builder(Command::new("echo").configure(|cmd| {
            cmd.arg("hello");
        }))
        .approval_handler(AlwaysDenyHandler)
        .spawn()
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
        assert!(
            err.to_string().contains("denied"),
            "error message should mention denial: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_approval_handler_closure() {
        let (proc, _stderr) = TokioChildProcess::builder(Command::new("echo").configure(|cmd| {
            cmd.arg("test");
        }))
        .approval_handler(
            |info: CommandInfo| -> futures::future::BoxFuture<'static, std::io::Result<bool>> {
                assert_eq!(info.program, "echo");
                assert_eq!(info.args, vec![OsString::from("test")]);
                Box::pin(async { Ok(true) })
            },
        )
        .spawn()
        .await
        .expect("spawn should succeed");

        assert!(proc.id().is_some());
    }

    #[tokio::test]
    async fn test_command_info_display() {
        let info = CommandInfo {
            program: OsString::from("npx"),
            args: vec![
                OsString::from("-y"),
                OsString::from("@modelcontextprotocol/server-everything"),
            ],
            envs: vec![],
            working_dir: Some(PathBuf::from("/tmp")),
        };
        let display = format!("{info}");
        assert!(display.contains("npx"));
        assert!(display.contains("-y"));
        assert!(display.contains("@modelcontextprotocol/server-everything"));
        assert!(display.contains("/tmp"));
    }

    #[tokio::test]
    async fn test_command_info_from_command_wrap() {
        let cmd = Command::new("npx").configure(|cmd| {
            cmd.arg("-y")
                .arg("some-package")
                .env("FOO", "bar")
                .current_dir("/tmp");
        });
        let wrap: CommandWrap = cmd.into();
        let info = CommandInfo::from_command_wrap(&wrap);
        assert_eq!(info.program, "npx");
        assert_eq!(
            info.args,
            vec![OsString::from("-y"), OsString::from("some-package")]
        );
        assert!(
            info.envs
                .contains(&(OsString::from("FOO"), Some(OsString::from("bar"))))
        );
        assert_eq!(info.working_dir, Some(PathBuf::from("/tmp")));
    }

    #[tokio::test]
    async fn test_no_approval_handler_spawns_directly() {
        // Without an approval handler, spawn should still work (backward compat)
        let (proc, _stderr) = TokioChildProcess::builder(Command::new("echo").configure(|cmd| {
            cmd.arg("no-handler");
        }))
        .spawn()
        .await
        .expect("spawn without handler should succeed");

        assert!(proc.id().is_some());
    }
}
