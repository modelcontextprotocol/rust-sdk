use rmcp::{
    ServiceExt,
    transport::{SseServer, TokioChildProcess},
};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
mod common;
use common::calculator::Calculator;

const BIND_ADDRESS: &str = "127.0.0.1:8000";

#[tokio::test]
async fn test_with_python_client() -> anyhow::Result<()> {
    let _ = tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "debug".to_string().into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .try_init();
    tokio::process::Command::new("uv")
        .args(["pip", "install", "-r", "pyproject.toml"])
        .current_dir("tests/test_with_python")
        .spawn()?
        .wait()
        .await?;

    let ct = SseServer::serve(BIND_ADDRESS.parse()?)
        .await?
        .with_service(Calculator::default);

    let status = tokio::process::Command::new("uv")
        .arg("run")
        .arg("tests/test_with_python/client.py")
        .spawn()?
        .wait()
        .await?;
    assert!(status.success());
    ct.cancel();
    Ok(())
}

#[tokio::test]
async fn test_with_python_server() -> anyhow::Result<()> {
    let _ = tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "debug".to_string().into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .try_init();
    tokio::process::Command::new("uv")
        .args(["pip", "install", "-r", "pyproject.toml"])
        .current_dir("tests/test_with_python")
        .spawn()?
        .wait()
        .await?;
    let transport = TokioChildProcess::new(
        tokio::process::Command::new("uv")
            .arg("run")
            .arg("tests/test_with_python/server.py"),
    )?;

    let client = ().serve(transport).await?;
    let resources = client.list_all_resources().await?;
    tracing::info!("{:#?}", resources);
    let tools = client.list_all_tools().await?;
    tracing::info!("{:#?}", tools);
    client.cancel().await?;
    Ok(())
}
