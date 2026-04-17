#![cfg(all(feature = "transport-streamable-http-server", not(feature = "local")))]

use std::time::Duration;

use rmcp::{
    model::{ClientJsonRpcMessage, ClientRequest, PingRequest, RequestId},
    transport::streamable_http_server::session::{SessionManager, local::LocalSessionManager},
};

#[tokio::test]
async fn test_init_timeout_terminates_pre_init_session() -> anyhow::Result<()> {
    let mut manager = LocalSessionManager::default();
    manager.session_config.init_timeout = Some(Duration::from_millis(200));

    // Bind the transport so its drop-guard doesn't cancel the worker — we
    // want termination via init_timeout, not via cancellation.
    let (session_id, _transport) = manager.create_session().await?;

    tokio::time::sleep(Duration::from_millis(500)).await;

    let message = ClientJsonRpcMessage::request(
        ClientRequest::PingRequest(PingRequest::default()),
        RequestId::Number(1),
    );
    let result = manager.initialize_session(&session_id, message).await;

    assert!(
        result.is_err(),
        "expected worker to be dead; got: {result:?}"
    );

    Ok(())
}

#[tokio::test]
async fn test_init_timeout_none_keeps_worker_alive() -> anyhow::Result<()> {
    let mut manager = LocalSessionManager::default();
    manager.session_config.init_timeout = None;

    let (session_id, _transport) = manager.create_session().await?;

    tokio::time::sleep(Duration::from_millis(500)).await;

    let message = ClientJsonRpcMessage::request(
        ClientRequest::PingRequest(PingRequest::default()),
        RequestId::Number(1),
    );
    // Liveness probe: a live worker accepts the send then stalls waiting for
    // a handler response (none is wired up), tripping the outer timeout. A
    // dead worker would fail the send and return immediately.
    let probe = tokio::time::timeout(
        Duration::from_millis(200),
        manager.initialize_session(&session_id, message),
    )
    .await;

    assert!(
        probe.is_err(),
        "expected worker to be alive; got: {probe:?}"
    );

    Ok(())
}
