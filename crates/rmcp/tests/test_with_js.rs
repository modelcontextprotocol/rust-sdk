use rmcp::{
    ServiceExt,
    transport::{SseServer, TokioChildProcess},
};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
mod common;
use common::calculator::Calculator;

const BIND_ADDRESS: &str = "127.0.0.1:8000";

#[tokio::test]
async fn test_with_js_client() -> anyhow::Result<()> {
    let _ = tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "debug".to_string().into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .try_init();
    tokio::process::Command::new("npm")
        .arg("install")
        .current_dir("tests/test_with_js")
        .spawn()?
        .wait()
        .await?;

    let ct = SseServer::serve(BIND_ADDRESS.parse()?)
        .await?
        .with_service(Calculator::default);

    let exit_status = tokio::process::Command::new("node")
        .arg("tests/test_with_js/client.js")
        .spawn()?
        .wait()
        .await?;
    assert!(exit_status.success());
    ct.cancel();
    Ok(())
}

#[tokio::test]
async fn test_with_js_server() -> anyhow::Result<()> {
    let _ = tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "debug".to_string().into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .try_init();
    tokio::process::Command::new("npm")
        .arg("install")
        .current_dir("tests/test_with_js")
        .spawn()?
        .wait()
        .await?;
    let transport = TokioChildProcess::new(
        tokio::process::Command::new("node").arg("tests/test_with_js/server.js"),
    )?;

    let client = ().serve(transport).await?;
    let resources = client.list_all_resources().await?;
    tracing::info!("{:#?}", resources);
    let tools = client.list_all_tools().await?;
    tracing::info!("{:#?}", tools);

    client.cancel().await?;
    Ok(())
}
