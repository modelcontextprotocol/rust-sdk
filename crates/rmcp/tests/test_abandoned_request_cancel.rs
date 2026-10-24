//! A client that stops waiting for a request tells the server with
//! `notifications/cancelled`.
//!
//! The request helpers hide the request they send, and the MRTR helpers
//! (`call_tool`, `get_prompt`, `read_resource`) hide one per round, so the only
//! way a caller can abandon one is to drop the future. These tests drop it at
//! each point of a round and check what the server is told.

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use rmcp::{
    ClientHandler, ServerHandler,
    model::*,
    service::{
        NotificationContext, RequestContext, RoleClient, RoleServer, RunningService, serve_directly,
    },
};
use serde_json::json;
use tokio::sync::{Notify, mpsc};

const ABANDONED_REASON: &str = "request abandoned by the caller";
const WAIT: Duration = Duration::from_secs(5);

/// Tool, prompt and resource names select the behavior:
/// - `hang`: blocks until the request is cancelled;
/// - `ask_then_hang`: one elicitation round, then blocks on the retry;
/// - `ask_blocking`: one elicitation the client never answers;
/// - anything else completes at once.
///
/// `prompts/list` and `completion/complete` always block, to stand for the
/// requests that have no MRTR rounds.
#[derive(Clone)]
struct ProbeServer {
    /// Signalled when a request starts to block.
    hanging: Arc<Notify>,
    /// ID of the request that is blocking.
    hanging_id: Arc<Mutex<Option<RequestId>>>,
    /// Every `notifications/cancelled` the server receives.
    cancelled: mpsc::UnboundedSender<CancelledNotificationParam>,
}

impl ProbeServer {
    async fn hang(&self, context: RequestContext<RoleServer>) -> ErrorData {
        *self.hanging_id.lock().unwrap() = Some(context.id.clone());
        self.hanging.notify_one();
        context.ct.cancelled().await;
        ErrorData::internal_error("cancelled", None)
    }
}

fn elicitation(message: &str) -> InputRequiredResult {
    let mut requests = InputRequests::new();
    requests.insert(
        "answer".to_string(),
        InputRequest::Elicitation(ElicitRequest::new(
            ElicitRequestParams::FormElicitationParams {
                meta: None,
                message: message.into(),
                requested_schema: serde_json::from_value(json!({
                    "type": "object",
                    "properties": { "name": { "type": "string" } },
                    "required": ["name"]
                }))
                .unwrap(),
            },
        )),
    );
    InputRequiredResult::new(Some(requests), Some("state".into()))
}

impl ServerHandler for ProbeServer {
    fn get_info(&self) -> ServerConfig {
        let mut info = ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_prompts()
                .enable_resources()
                .build(),
        );
        info.protocol_version = ProtocolVersion::V_2026_07_28;
        info
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let answered = request.input_responses.is_some();
        match request.name.as_ref() {
            "hang" => Err(self.hang(context).await),
            "ask_then_hang" if answered => Err(self.hang(context).await),
            "ask_then_hang" => Ok(elicitation("Name?").into()),
            "ask_blocking" => Ok(elicitation("BLOCK").into()),
            _ => Ok(CallToolResult::success(vec![ContentBlock::text("done")]).into()),
        }
    }

    async fn get_prompt(
        &self,
        _request: GetPromptRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResponse, ErrorData> {
        Err(self.hang(context).await)
    }

    async fn read_resource(
        &self,
        _request: ReadResourceRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        Err(self.hang(context).await)
    }

    async fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, ErrorData> {
        Err(self.hang(context).await)
    }

    async fn complete(
        &self,
        _request: CompleteRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CompleteResult, ErrorData> {
        Err(self.hang(context).await)
    }

    async fn on_cancelled(
        &self,
        notification: CancelledNotificationParam,
        _context: NotificationContext<RoleServer>,
    ) {
        let _ = self.cancelled.send(notification);
    }
}

/// Answers every elicitation except `"BLOCK"`, which it never answers.
#[derive(Clone)]
struct ProbeClient {
    /// Signalled when the blocking elicitation starts.
    eliciting: Arc<Notify>,
}

impl ClientHandler for ProbeClient {
    async fn create_elicitation(
        &self,
        request: ElicitRequestParams,
        _context: RequestContext<RoleClient>,
    ) -> Result<ElicitResult, ErrorData> {
        if let ElicitRequestParams::FormElicitationParams { message, .. } = &request
            && message == "BLOCK"
        {
            self.eliciting.notify_one();
            std::future::pending::<()>().await;
        }
        Ok(ElicitResult::new(ElicitationAction::Accept).with_content(json!({ "name": "Ferris" })))
    }
}

struct Probe {
    client: RunningService<RoleClient, ProbeClient>,
    hanging: Arc<Notify>,
    hanging_id: Arc<Mutex<Option<RequestId>>>,
    eliciting: Arc<Notify>,
    cancelled: mpsc::UnboundedReceiver<CancelledNotificationParam>,
}

impl Probe {
    /// The single cancellation the server should have received.
    async fn expect_cancelled(&mut self) -> CancelledNotificationParam {
        tokio::time::timeout(WAIT, self.cancelled.recv())
            .await
            .expect("server received no notifications/cancelled")
            .expect("server dropped the cancellation channel")
    }

    /// Runs a request to completion after the point under test, so that any
    /// cancellation sent before it has reached the server, then checks that
    /// none did.
    async fn expect_no_cancellation(&mut self) -> anyhow::Result<()> {
        self.client
            .call_tool(CallToolRequestParams::new("noop"))
            .await?;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            self.cancelled.try_recv().is_err(),
            "server received an unexpected notifications/cancelled"
        );
        Ok(())
    }
}

/// Runs `body` inside a `LocalSet` so `spawn_local` (used when the `local`
/// feature is active) is available, wiring up a connected client/server pair.
async fn with_probe<F, Fut>(body: F) -> anyhow::Result<()>
where
    F: FnOnce(Probe) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<()>>,
{
    tokio::task::LocalSet::new()
        .run_until(async move {
            let (cancelled_tx, cancelled) = mpsc::unbounded_channel();
            let server = ProbeServer {
                hanging: Arc::new(Notify::new()),
                hanging_id: Arc::new(Mutex::new(None)),
                cancelled: cancelled_tx,
            };
            let hanging = server.hanging.clone();
            let hanging_id = server.hanging_id.clone();
            let eliciting = Arc::new(Notify::new());

            let (server_transport, client_transport) = tokio::io::duplex(8192);
            let server_peer_info = InitializeRequestParams::new(
                ClientCapabilities::builder().enable_elicitation().build(),
                Implementation::new("abandoned-request-test-client", "0.0.0"),
            )
            .with_protocol_version(ProtocolVersion::V_2026_07_28);
            let mut client_peer_info =
                InitializeResult::new(ServerCapabilities::builder().enable_tools().build());
            client_peer_info.protocol_version = ProtocolVersion::V_2026_07_28;

            let server_task = tokio::task::spawn_local(async move {
                let running = serve_directly::<RoleServer, _, _, _, _>(
                    server,
                    server_transport,
                    Some(server_peer_info),
                );
                running.waiting().await?;
                anyhow::Ok(())
            });
            let client = serve_directly::<RoleClient, _, _, _, _>(
                ProbeClient {
                    eliciting: eliciting.clone(),
                },
                client_transport,
                Some(client_peer_info.into()),
            );

            let result = body(Probe {
                client,
                hanging,
                hanging_id,
                eliciting,
                cancelled,
            })
            .await;

            server_task.abort();
            result
        })
        .await
}

/// Polls `call` until `reached` fires, then drops it: the caller gave up at
/// that point.
async fn abandon_when<T>(call: impl Future<Output = T>, reached: &Notify) {
    let mut call = Box::pin(call);
    tokio::select! {
        _ = &mut call => panic!("the call completed before it could be abandoned"),
        reached = tokio::time::timeout(WAIT, reached.notified()) => {
            reached.expect("the call never reached the point to abandon it at");
        }
    }
    drop(call);
}

#[tokio::test(flavor = "current_thread")]
async fn abandoned_tool_call_is_cancelled() -> anyhow::Result<()> {
    with_probe(|mut probe| async move {
        abandon_when(
            probe.client.call_tool(CallToolRequestParams::new("hang")),
            &probe.hanging,
        )
        .await;

        let cancelled = probe.expect_cancelled().await;
        assert_eq!(cancelled.request_id, *probe.hanging_id.lock().unwrap());
        assert_eq!(cancelled.reason.as_deref(), Some(ABANDONED_REASON));
        Ok(())
    })
    .await
}

#[tokio::test(flavor = "current_thread")]
async fn abandoned_retry_round_cancels_that_round() -> anyhow::Result<()> {
    with_probe(|mut probe| async move {
        // The first round completes with `input_required`; the call is dropped
        // while the retry is in flight, so the retry is the request to cancel.
        abandon_when(
            probe
                .client
                .call_tool(CallToolRequestParams::new("ask_then_hang")),
            &probe.hanging,
        )
        .await;

        let cancelled = probe.expect_cancelled().await;
        assert_eq!(cancelled.request_id, *probe.hanging_id.lock().unwrap());
        probe.expect_no_cancellation().await
    })
    .await
}

#[tokio::test(flavor = "current_thread")]
async fn abandoned_prompt_and_resource_requests_are_cancelled() -> anyhow::Result<()> {
    with_probe(|mut probe| async move {
        abandon_when(
            probe.client.get_prompt(GetPromptRequestParams::new("hang")),
            &probe.hanging,
        )
        .await;
        let cancelled = probe.expect_cancelled().await;
        assert_eq!(cancelled.request_id, *probe.hanging_id.lock().unwrap());

        abandon_when(
            probe
                .client
                .read_resource(ReadResourceRequestParams::new("test://hang")),
            &probe.hanging,
        )
        .await;
        let cancelled = probe.expect_cancelled().await;
        assert_eq!(cancelled.request_id, *probe.hanging_id.lock().unwrap());
        Ok(())
    })
    .await
}

#[tokio::test(flavor = "current_thread")]
async fn abandoned_requests_without_mrtr_rounds_are_cancelled() -> anyhow::Result<()> {
    with_probe(|mut probe| async move {
        abandon_when(probe.client.list_prompts(None), &probe.hanging).await;
        let cancelled = probe.expect_cancelled().await;
        assert_eq!(cancelled.request_id, *probe.hanging_id.lock().unwrap());
        assert_eq!(cancelled.reason.as_deref(), Some(ABANDONED_REASON));

        let complete = CompleteRequestParams::new(
            Reference::for_prompt("anything"),
            ArgumentInfo::new("argument", "va"),
        );
        abandon_when(probe.client.complete(complete), &probe.hanging).await;
        let cancelled = probe.expect_cancelled().await;
        assert_eq!(cancelled.request_id, *probe.hanging_id.lock().unwrap());
        probe.expect_no_cancellation().await
    })
    .await
}

#[tokio::test(flavor = "current_thread")]
async fn abandoning_during_local_input_handling_cancels_nothing() -> anyhow::Result<()> {
    with_probe(|mut probe| async move {
        // The round's request has completed with `input_required` and the retry
        // has not been sent: no request is in flight while the client handler
        // works on the elicitation, so there is nothing to cancel.
        abandon_when(
            probe
                .client
                .call_tool(CallToolRequestParams::new("ask_blocking")),
            &probe.eliciting,
        )
        .await;

        probe.expect_no_cancellation().await
    })
    .await
}

#[tokio::test(flavor = "current_thread")]
async fn completed_call_cancels_nothing() -> anyhow::Result<()> {
    with_probe(|mut probe| async move {
        let result = probe
            .client
            .call_tool(CallToolRequestParams::new("noop"))
            .await?;
        assert_eq!(result.content[0].as_text().unwrap().text, "done");

        probe.expect_no_cancellation().await
    })
    .await
}
