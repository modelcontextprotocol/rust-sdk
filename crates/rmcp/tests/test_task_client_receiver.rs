//! End-to-end tests proving that task-management RPCs (`tasks/get`,
//! `tasks/list`, `tasks/result`, `tasks/cancel`) sent from a server to a
//! client are dispatched to the appropriate `ClientHandler` methods and
//! return results that the server receives as the correct `ClientResult`
//! variant.
//!
//! Companion tests for the server-as-receiver path already exist; these
//! round out the bidirectional coverage required by SEP-1686.

#![cfg(not(feature = "local"))]
use std::sync::Arc;

use rmcp::{
    ClientHandler, ServerHandler, ServiceExt,
    model::{
        CancelTaskParams, CancelTaskRequest, CancelTaskResult, ClientResult, GetTaskInfoParams,
        GetTaskInfoRequest, GetTaskPayloadResult, GetTaskResult, GetTaskResultParams,
        GetTaskResultRequest, ListTasksRequest, ListTasksResult, PaginatedRequestParams,
        ServerRequest, Task, TaskStatus,
    },
};
use serde_json::json;
use tokio::sync::{Mutex, Notify};

/// Shared bookkeeping for the client handler: records which method was
/// invoked with which task id, so each test can assert on it.
#[derive(Default)]
struct ClientState {
    last_get_task_id: Option<String>,
    last_result_task_id: Option<String>,
    last_cancel_task_id: Option<String>,
    list_called: bool,
}

struct TaskClient {
    state: Arc<Mutex<ClientState>>,
    received: Arc<Notify>,
}

impl ClientHandler for TaskClient {
    async fn get_task_info(
        &self,
        request: GetTaskInfoParams,
        _context: rmcp::service::RequestContext<rmcp::RoleClient>,
    ) -> Result<GetTaskResult, rmcp::ErrorData> {
        self.state.lock().await.last_get_task_id = Some(request.task_id.clone());
        self.received.notify_one();
        Ok(GetTaskResult {
            meta: None,
            task: Task::new(
                request.task_id,
                TaskStatus::Working,
                "2025-11-25T10:30:00Z".into(),
                "2025-11-25T10:30:00Z".into(),
            ),
        })
    }

    async fn list_tasks(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: rmcp::service::RequestContext<rmcp::RoleClient>,
    ) -> Result<ListTasksResult, rmcp::ErrorData> {
        self.state.lock().await.list_called = true;
        self.received.notify_one();
        Ok(ListTasksResult::new(vec![Task::new(
            "task-42".to_string(),
            TaskStatus::Working,
            "2025-11-25T10:30:00Z".into(),
            "2025-11-25T10:30:00Z".into(),
        )]))
    }

    async fn get_task_result(
        &self,
        request: GetTaskResultParams,
        _context: rmcp::service::RequestContext<rmcp::RoleClient>,
    ) -> Result<GetTaskPayloadResult, rmcp::ErrorData> {
        self.state.lock().await.last_result_task_id = Some(request.task_id);
        self.received.notify_one();
        Ok(GetTaskPayloadResult::new(json!({ "ok": true })))
    }

    async fn cancel_task(
        &self,
        request: CancelTaskParams,
        _context: rmcp::service::RequestContext<rmcp::RoleClient>,
    ) -> Result<CancelTaskResult, rmcp::ErrorData> {
        self.state.lock().await.last_cancel_task_id = Some(request.task_id.clone());
        self.received.notify_one();
        Ok(CancelTaskResult {
            meta: None,
            task: Task::new(
                request.task_id,
                TaskStatus::Cancelled,
                "2025-11-25T10:30:00Z".into(),
                "2025-11-25T10:30:00Z".into(),
            ),
        })
    }
}

/// Signal we fire when the server finishes its outbound RPC so the test
/// can assert on the response shape.
struct ServerCompletion {
    done: Arc<Notify>,
    last_response: Arc<Mutex<Option<Result<ClientResult, String>>>>,
}

/// A server that, on initialize, fires a single `ServerRequest` (parameterised
/// externally via a closure) at the client and stashes the response.
struct RequestingServer {
    request: Arc<Mutex<Option<ServerRequest>>>,
    completion: ServerCompletion,
}

impl ServerHandler for RequestingServer {
    async fn on_initialized(&self, context: rmcp::service::NotificationContext<rmcp::RoleServer>) {
        let peer = context.peer.clone();
        let request = self.request.lock().await.take();
        let done = self.completion.done.clone();
        let last_response = self.completion.last_response.clone();
        tokio::spawn(async move {
            let Some(req) = request else {
                *last_response.lock().await = Some(Err("no request".into()));
                done.notify_one();
                return;
            };
            let outcome = peer
                .send_request(req)
                .await
                .map_err(|e| format!("send_request failed: {e}"));
            *last_response.lock().await = Some(outcome);
            done.notify_one();
        });
    }
}

async fn run_server_request(request: ServerRequest) -> (Arc<Mutex<ClientState>>, ClientResult) {
    let _ = tracing_subscriber::fmt::try_init();

    let (server_transport, client_transport) = tokio::io::duplex(4096);

    let completion = ServerCompletion {
        done: Arc::new(Notify::new()),
        last_response: Arc::new(Mutex::new(None)),
    };
    let server_done = completion.done.clone();
    let server_response = completion.last_response.clone();
    tokio::spawn({
        let request_slot = Arc::new(Mutex::new(Some(request)));
        async move {
            let server = RequestingServer {
                request: request_slot,
                completion,
            }
            .serve(server_transport)
            .await?;
            server.waiting().await?;
            anyhow::Ok(())
        }
    });

    let state = Arc::new(Mutex::new(ClientState::default()));
    let received = Arc::new(Notify::new());
    let client = TaskClient {
        state: state.clone(),
        received: received.clone(),
    }
    .serve(client_transport)
    .await
    .expect("client serve");

    tokio::time::timeout(std::time::Duration::from_secs(5), received.notified())
        .await
        .expect("client handler fired");
    tokio::time::timeout(std::time::Duration::from_secs(5), server_done.notified())
        .await
        .expect("server got response");

    let outcome = server_response
        .lock()
        .await
        .take()
        .expect("server outcome set");
    let response = outcome.expect("server request succeeded");

    client.cancel().await.ok();
    (state, response)
}

#[tokio::test]
async fn tasks_get_reaches_client_handler() {
    let request = ServerRequest::GetTaskInfoRequest(GetTaskInfoRequest::new(GetTaskInfoParams {
        meta: None,
        task_id: "task-abc".into(),
    }));
    let (state, response) = run_server_request(request).await;

    assert_eq!(
        state.lock().await.last_get_task_id.as_deref(),
        Some("task-abc")
    );
    match response {
        ClientResult::GetTaskResult(r) => {
            assert_eq!(r.task.task_id, "task-abc");
            assert_eq!(r.task.status, TaskStatus::Working);
        }
        other => panic!("unexpected variant: {other:?}"),
    }
}

#[tokio::test]
async fn tasks_list_reaches_client_handler() {
    let request = ServerRequest::ListTasksRequest(ListTasksRequest::default());
    let (state, response) = run_server_request(request).await;

    assert!(state.lock().await.list_called);
    match response {
        ClientResult::ListTasksResult(r) => {
            assert_eq!(r.tasks.len(), 1);
            assert_eq!(r.tasks[0].task_id, "task-42");
        }
        other => panic!("unexpected variant: {other:?}"),
    }
}

#[tokio::test]
async fn tasks_result_reaches_client_handler() {
    let request =
        ServerRequest::GetTaskResultRequest(GetTaskResultRequest::new(GetTaskResultParams {
            meta: None,
            task_id: "task-xyz".into(),
        }));
    let (state, response) = run_server_request(request).await;

    assert_eq!(
        state.lock().await.last_result_task_id.as_deref(),
        Some("task-xyz")
    );
    // GetTaskPayloadResult has a custom Deserialize that always fails (see
    // crates/rmcp/src/model/task.rs) so the payload surfaces as the
    // catch-all CustomResult on the wire. This matches the existing design
    // on the server-as-receiver path.
    match response {
        ClientResult::CustomResult(r) => {
            assert_eq!(r.0, json!({ "ok": true }));
        }
        other => panic!("unexpected variant: {other:?}"),
    }
}

#[tokio::test]
async fn tasks_cancel_reaches_client_handler() {
    let request = ServerRequest::CancelTaskRequest(CancelTaskRequest::new(CancelTaskParams {
        meta: None,
        task_id: "task-cancelme".into(),
    }));
    let (state, response) = run_server_request(request).await;

    assert_eq!(
        state.lock().await.last_cancel_task_id.as_deref(),
        Some("task-cancelme")
    );
    // CancelTaskResult and GetTaskResult share the same JSON shape
    // (`Result + flattened Task`); the untagged ClientResult enum picks
    // the first match, which is GetTaskResult. Callers distinguish by
    // knowing which request they sent rather than by inspecting the
    // response variant. This mirrors the existing behavior on the server
    // side.
    match response {
        ClientResult::GetTaskResult(r) => {
            assert_eq!(r.task.task_id, "task-cancelme");
            assert_eq!(r.task.status, TaskStatus::Cancelled);
        }
        other => panic!("unexpected variant: {other:?}"),
    }
}

#[tokio::test]
async fn default_handler_returns_method_not_found() {
    use rmcp::model::ErrorCode;
    let _ = tracing_subscriber::fmt::try_init();

    let (server_transport, client_transport) = tokio::io::duplex(4096);
    let completion = ServerCompletion {
        done: Arc::new(Notify::new()),
        last_response: Arc::new(Mutex::new(None)),
    };
    let done = completion.done.clone();
    let response = completion.last_response.clone();
    tokio::spawn({
        let request_slot = Arc::new(Mutex::new(Some(ServerRequest::GetTaskInfoRequest(
            GetTaskInfoRequest::new(GetTaskInfoParams {
                meta: None,
                task_id: "whatever".into(),
            }),
        ))));
        async move {
            let server = RequestingServer {
                request: request_slot,
                completion,
            }
            .serve(server_transport)
            .await?;
            server.waiting().await?;
            anyhow::Ok(())
        }
    });

    // Use the empty-unit client, which relies on the default trait impls.
    let client = ().serve(client_transport).await.expect("client");

    tokio::time::timeout(std::time::Duration::from_secs(5), done.notified())
        .await
        .expect("server got response");

    let outcome = response.lock().await.take().expect("response set");
    let err_msg = match outcome {
        Ok(other) => panic!("expected method-not-found error, got: {other:?}"),
        Err(s) => s,
    };
    // The ServiceError Display surfaces the inner McpError, which carries
    // the METHOD_NOT_FOUND code.
    assert!(
        err_msg.contains(&ErrorCode::METHOD_NOT_FOUND.0.to_string())
            || err_msg.to_lowercase().contains("method not found")
            || err_msg.to_lowercase().contains("tasks/get"),
        "expected method-not-found style error, got: {err_msg}"
    );

    client.cancel().await.ok();
}
