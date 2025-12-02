use std::process::Stdio;

use rmcp::{
    ServiceExt,
    transport::{ConfigureCommandExt, TokioChildProcess},
};
use tokio::io::AsyncReadExt;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
mod common;

async fn init() -> anyhow::Result<()> {
    let _ = tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "debug".to_string().into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .try_init();
    tokio::process::Command::new("uv")
        .args(["sync"])
        .current_dir("tests/test_with_python")
        .spawn()?
        .wait()
        .await?;
    Ok(())
}

#[tokio::test]
async fn test_with_python_server() -> anyhow::Result<()> {
    init().await?;

    let transport = TokioChildProcess::new(tokio::process::Command::new("uv").configure(|cmd| {
        cmd.arg("run")
            .arg("server.py")
            .current_dir("tests/test_with_python");
    }))?;

    let client = ().serve(transport).await?;
    let resources = client.list_all_resources().await?;
    tracing::info!("{:#?}", resources);
    let tools = client.list_all_tools().await?;
    tracing::info!("{:#?}", tools);
    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn test_with_python_server_stderr() -> anyhow::Result<()> {
    init().await?;

    let (transport, stderr) =
        TokioChildProcess::builder(tokio::process::Command::new("uv").configure(|cmd| {
            cmd.arg("run")
                .arg("server.py")
                .current_dir("tests/test_with_python");
        }))
        .stderr(Stdio::piped())
        .spawn()?;

    let mut stderr = stderr.expect("stderr must be piped");

    let stderr_task = tokio::spawn(async move {
        let mut buffer = String::new();
        stderr.read_to_string(&mut buffer).await?;
        Ok::<_, std::io::Error>(buffer)
    });

    let client = ().serve(transport).await?;
    let _ = client.list_all_resources().await?;
    let _ = client.list_all_tools().await?;
    client.cancel().await?;

    let stderr_output = stderr_task.await??;
    assert!(stderr_output.contains("server starting up..."));

    Ok(())
}
