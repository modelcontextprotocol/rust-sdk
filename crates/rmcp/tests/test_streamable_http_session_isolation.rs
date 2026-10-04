#![cfg(all(feature = "transport-streamable-http-server", not(feature = "local")))]

use std::{sync::Arc, time::Duration};

use rmcp::{
    model::{ClientJsonRpcMessage, ClientRequest, PingRequest, RequestId},
    transport::streamable_http_server::session::{
        SessionId, SessionManager,
        local::{LocalSessionManager, SessionConfig},
    },
};
use rstest::rstest;
use tokio::task::JoinHandle;

const PROBE_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StuckCall {
    Initialize,
    CreateStream,
    CreateStandaloneStream,
    Resume,
    AcceptMessage,
}

fn ping() -> ClientJsonRpcMessage {
    ClientJsonRpcMessage::request(
        ClientRequest::PingRequest(PingRequest::default()),
        RequestId::Number(1),
    )
}

fn spawn_call(
    manager: &Arc<LocalSessionManager>,
    id: &SessionId,
    call: StuckCall,
) -> JoinHandle<()> {
    let manager = manager.clone();
    let id = id.clone();
    tokio::spawn(async move {
        match call {
            StuckCall::Initialize => {
                let _ = manager.initialize_session(&id, ping()).await;
            }
            StuckCall::CreateStream => {
                let _ = manager.create_stream(&id, ping()).await;
            }
            StuckCall::CreateStandaloneStream => {
                let _ = manager.create_standalone_stream(&id).await;
            }
            StuckCall::Resume => {
                let _ = manager.resume(&id, "0".to_owned()).await;
            }
            StuckCall::AcceptMessage => {
                // The worker no longer reads its event channel, so the push
                // after the channel is full waits.
                for _ in 0..=SessionConfig::DEFAULT_CHANNEL_CAPACITY {
                    let _ = manager.accept_message(&id, ping()).await;
                }
            }
        }
    })
}

/// While a call waits on the worker of one session, the manager must still
/// create new sessions and look up other sessions.
#[rstest]
#[case::initialize(StuckCall::Initialize)]
#[case::create_stream(StuckCall::CreateStream)]
#[case::create_standalone_stream(StuckCall::CreateStandaloneStream)]
#[case::resume(StuckCall::Resume)]
#[case::accept_message(StuckCall::AcceptMessage)]
#[tokio::test]
async fn waiting_on_one_session_does_not_block_other_sessions(
    #[case] call: StuckCall,
) -> anyhow::Result<()> {
    let manager = Arc::new(LocalSessionManager::default());
    let (other_id, _other_transport) = manager.create_session().await?;
    // Nothing serves this transport, so the worker never answers initialize
    // and stops reading session events. Keep the transport bound: dropping it
    // cancels the worker.
    let (stuck_id, _stuck_transport) = manager.create_session().await?;

    let mut stuck = vec![spawn_call(&manager, &stuck_id, StuckCall::Initialize)];
    if call != StuckCall::Initialize {
        tokio::time::sleep(Duration::from_millis(50)).await;
        stuck.push(spawn_call(&manager, &stuck_id, call));
    }
    tokio::time::sleep(Duration::from_millis(100)).await;

    // A new client connects while the call is waiting...
    let create = tokio::spawn({
        let manager = manager.clone();
        async move { manager.create_session().await.map(|(id, _)| id) }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;

    // ...and a request arrives on another session.
    let lookup = tokio::time::timeout(PROBE_TIMEOUT, manager.has_session(&other_id)).await;
    assert!(
        matches!(lookup, Ok(Ok(true))),
        "has_session blocked: {lookup:?}"
    );
    let created = tokio::time::timeout(PROBE_TIMEOUT, create).await;
    assert!(
        matches!(created, Ok(Ok(Ok(_)))),
        "create_session blocked: {created:?}"
    );

    for task in stuck {
        assert!(
            !task.is_finished(),
            "the call on the stuck session should still be waiting"
        );
        task.abort();
    }
    Ok(())
}
