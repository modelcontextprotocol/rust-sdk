use axum::Router;
use rmcp::{
    ServiceExt,
    transport::{ConfigureCommandExt, SseServer, TokioChildProcess, sse_server::SseServerConfig},
};
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
mod common;
use common::calculator::Calculator;

async fn init() -> anyhow::Result<()> {
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
    Ok(())
}

#[tokio::test]
async fn test_with_python_client() -> anyhow::Result<()> {
    init().await?;

    const BIND_ADDRESS: &str = "127.0.0.1:8000";

    let ct = SseServer::serve(BIND_ADDRESS.parse()?)
        .await?
        .with_service(Calculator::default);

    let status = tokio::process::Command::new("uv")
        .arg("run")
        .arg("tests/test_with_python/client.py")
        .arg(format!("http://{BIND_ADDRESS}/sse"))
        .spawn()?
        .wait()
        .await?;
    assert!(status.success());
    ct.cancel();
    Ok(())
}

/// Test the SSE server in a nested Axum router.
#[tokio::test]
async fn test_nested_with_python_client() -> anyhow::Result<()> {
    init().await?;

    const BIND_ADDRESS: &str = "127.0.0.1:8001";

    // Create an SSE router
    let sse_config = SseServerConfig {
        bind: BIND_ADDRESS.parse()?,
        sse_path: "/sse".to_string(),
        post_path: "/message".to_string(),
        ct: CancellationToken::new(),
        sse_keep_alive: None,
    };

    let listener = tokio::net::TcpListener::bind(&sse_config.bind).await?;

    let (sse_server, sse_router) = SseServer::new(sse_config);
    let ct = sse_server.with_service(Calculator::default);

    let main_router = Router::new().nest("/nested", sse_router);

    let server_ct = ct.clone();
    let server = axum::serve(listener, main_router).with_graceful_shutdown(async move {
        server_ct.cancelled().await;
        tracing::info!("sse server cancelled");
    });

    tokio::spawn(async move {
        let _ = server.await;
        tracing::info!("sse server shutting down");
    });

    // Spawn the process with timeout, as failure to access the '/message' URL
    // causes the client to never exit.
    let status = timeout(
        tokio::time::Duration::from_secs(5),
        tokio::process::Command::new("uv")
            .arg("run")
            .arg("tests/test_with_python/client.py")
            .arg(format!("http://{BIND_ADDRESS}/nested/sse"))
            .spawn()?
            .wait(),
    )
    .await?;
    assert!(status?.success());
    ct.cancel();
    Ok(())
}

#[tokio::test]
async fn test_with_python_server() -> anyhow::Result<()> {
    init().await?;

    let transport = TokioChildProcess::new(tokio::process::Command::new("uv").configure(|cmd| {
        cmd.arg("run").arg("tests/test_with_python/server.py");
    }))?;

    let client = ().serve(transport).await?;
    let resources = client.list_all_resources().await?;
    tracing::info!("{:#?}", resources);
    let tools = client.list_all_tools().await?;
    tracing::info!("{:#?}", tools);
    client.cancel().await?;
    Ok(())
}
