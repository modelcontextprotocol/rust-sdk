//! Behavior and edge-case coverage for SEP-2322 multi round-trip requests (MRTR).
//!
//! These tests drive a real client/server pair over an in-memory duplex stream
//! and exercise the auto fulfill/retry loop, the manual `*_once` escape hatch,
//! and the server-side version gating.

// Sampling/Roots are SEP-2577-deprecated but still used to model MRTR input requests.
#![allow(deprecated)]
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use rmcp::{
    ClientHandler, ServerHandler,
    model::*,
    service::{RequestContext, RoleClient, RoleServer, ServiceError, serve_directly},
};
use serde_json::json;

/// A `requestState` value with characters that must survive a byte-exact echo:
/// dots (the codec delimiter), base64 punctuation, whitespace, and quotes.
const TRICKY_STATE: &str = "st.ate/with+special=chars and spaces \"quotes\"\n\ttab";

// =============================================================================
// Test handlers
// =============================================================================

/// A stateless MRTR server whose behavior is selected by the tool/prompt/resource
/// name. Round progression is derived entirely from `request_state` and
/// `input_responses`, as required by the stateless MRTR pattern.
#[derive(Clone, Default)]
struct MrtrServer {
    calls: Arc<AtomicUsize>,
}

fn elicitation_request(message: &str) -> InputRequest {
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
    ))
}

fn sampling_request() -> InputRequest {
    InputRequest::CreateMessage(CreateMessageRequest::new(CreateMessageRequestParams::new(
        vec![SamplingMessage::user_text("What is the capital of France?")],
        100,
    )))
}

fn roots_request() -> InputRequest {
    InputRequest::ListRoots(ListRootsRequest::default())
}

fn single_elicitation(state: &str) -> InputRequiredResult {
    let mut requests = InputRequests::new();
    requests.insert("answer".to_string(), elicitation_request("Name?"));
    InputRequiredResult::new(Some(requests), Some(state.into()))
}

impl MrtrServer {
    fn call_tool_impl(
        &self,
        request: CallToolRequestParams,
    ) -> Result<CallToolResponse, ErrorData> {
        let responses = request.input_responses.as_ref();
        let state = request.request_state.as_deref();
        match request.name.as_ref() {
            // Single round: one elicitation, then complete.
            "single" => match responses {
                None => Ok(single_elicitation("state-single").into()),
                Some(map) => {
                    if state != Some("state-single") {
                        return Err(ErrorData::internal_error("request_state not echoed", None));
                    }
                    let answer = map
                        .get("answer")
                        .ok_or_else(|| ErrorData::internal_error("missing answer", None))?;
                    if answer["action"] != "accept" || answer["content"]["name"] != "Ferris" {
                        return Err(ErrorData::internal_error("unexpected elicit result", None));
                    }
                    Ok(CallToolResult::success(vec![ContentBlock::text("done")]).into())
                }
            },
            // Two elicitation rounds before completing.
            "multi_round" => match state {
                None => Ok(single_elicitation("round-1").into()),
                Some("round-1") => Ok(single_elicitation("round-2").into()),
                Some("round-2") => {
                    Ok(CallToolResult::success(vec![ContentBlock::text("multi-done")]).into())
                }
                Some(other) => Err(ErrorData::internal_error(
                    format!("unexpected round state {other:?}"),
                    None,
                )),
            },
            // Several input requests fulfilled concurrently in a single round.
            "multi_request" => match responses {
                None => {
                    let mut requests = InputRequests::new();
                    requests.insert("form".to_string(), elicitation_request("Name?"));
                    requests.insert("sample".to_string(), sampling_request());
                    requests.insert("roots".to_string(), roots_request());
                    Ok(InputRequiredResult::new(Some(requests), Some("multi-req".into())).into())
                }
                Some(map) => {
                    for key in ["form", "sample", "roots"] {
                        if !map.contains_key(key) {
                            return Err(ErrorData::internal_error(
                                format!("missing response for {key}"),
                                None,
                            ));
                        }
                    }
                    Ok(CallToolResult::success(vec![ContentBlock::text("multi-req-done")]).into())
                }
            },
            // State-only load shedding: two state-only rounds, then complete.
            "state_only" => match state {
                None => Ok(InputRequiredResult::from_request_state("so-1").into()),
                Some("so-1") => Ok(InputRequiredResult::from_request_state("so-2").into()),
                Some("so-2") => {
                    Ok(CallToolResult::success(vec![ContentBlock::text("state-done")]).into())
                }
                Some(other) => Err(ErrorData::internal_error(
                    format!("unexpected state {other:?}"),
                    None,
                )),
            },
            // Never completes: used to exercise the max-rounds cap.
            "loops" => Ok(single_elicitation("loop").into()),
            // Triggers a failure inside the client's elicitation handler.
            "handler_error" => {
                let mut requests = InputRequests::new();
                requests.insert("answer".to_string(), elicitation_request("FAIL"));
                Ok(InputRequiredResult::new(Some(requests), Some("state".into())).into())
            }
            // Verifies the client echoes `request_state` byte-for-byte.
            "echo_state" => match responses {
                None => {
                    let mut requests = InputRequests::new();
                    requests.insert("answer".to_string(), elicitation_request("Name?"));
                    Ok(InputRequiredResult::new(Some(requests), Some(TRICKY_STATE.into())).into())
                }
                Some(_) => {
                    if state != Some(TRICKY_STATE) {
                        return Err(ErrorData::internal_error(
                            "request_state was not echoed byte-exact",
                            None,
                        ));
                    }
                    Ok(CallToolResult::success(vec![ContentBlock::text("echo-ok")]).into())
                }
            },
            _ => Ok(CallToolResult::success(vec![ContentBlock::text("noop")]).into()),
        }
    }
}

impl ServerHandler for MrtrServer {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_prompts()
                .build(),
        );
        info.protocol_version = ProtocolVersion::V_2026_07_28;
        info
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.call_tool_impl(request)
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResponse, ErrorData> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        match request.request_state.as_deref() {
            None => Ok(single_elicitation("prompt-1").into()),
            Some("prompt-1") => Ok(GetPromptResult::new(vec![PromptMessage::new_text(
                Role::Assistant,
                "prompt-done",
            )])
            .into()),
            Some(other) => Err(ErrorData::internal_error(
                format!("unexpected prompt state {other:?}"),
                None,
            )),
        }
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        match request.request_state.as_deref() {
            None => Ok(single_elicitation("res-1").into()),
            Some("res-1") => Ok(ReadResourceResult::new(vec![ResourceContents::text(
                "resource-done",
                request.uri,
            )])
            .into()),
            Some(other) => Err(ErrorData::internal_error(
                format!("unexpected resource state {other:?}"),
                None,
            )),
        }
    }
}

/// A client that fulfills every kind of MRTR input request. Elicitation fails
/// deliberately when the prompt message is `"FAIL"`.
#[derive(Clone, Default)]
struct MrtrClient;

impl ClientHandler for MrtrClient {
    async fn create_elicitation(
        &self,
        request: ElicitRequestParams,
        _context: RequestContext<RoleClient>,
    ) -> Result<ElicitResult, ErrorData> {
        if let ElicitRequestParams::FormElicitationParams { message, .. } = &request {
            if message == "FAIL" {
                return Err(ErrorData::internal_error(
                    "elicitation handler failed",
                    None,
                ));
            }
        }
        Ok(ElicitResult::new(ElicitationAction::Accept).with_content(json!({ "name": "Ferris" })))
    }

    async fn create_message(
        &self,
        _request: CreateMessageRequestParams,
        _context: RequestContext<RoleClient>,
    ) -> Result<CreateMessageResult, ErrorData> {
        Ok(CreateMessageResult::new(
            SamplingMessage::assistant_text("Paris."),
            "test-model".into(),
        )
        .with_stop_reason(CreateMessageResult::STOP_REASON_END_TURN))
    }

    async fn list_roots(
        &self,
        _context: RequestContext<RoleClient>,
    ) -> Result<ListRootsResult, ErrorData> {
        Ok(ListRootsResult::new(vec![Root::new("file:///workspace")]))
    }
}

// =============================================================================
// Harness
// =============================================================================

fn client_info(protocol_version: ProtocolVersion) -> ClientInfo {
    ClientInfo::new(
        ClientCapabilities::builder().enable_elicitation().build(),
        Implementation::new("mrtr-test-client", "0.0.0"),
    )
    .with_protocol_version(protocol_version)
}

fn server_info_2026() -> ServerInfo {
    let mut info = ServerInfo::new(ServerCapabilities::builder().enable_tools().build());
    info.protocol_version = ProtocolVersion::V_2026_07_28;
    info
}

/// Runs `body` inside a `LocalSet` so `spawn_local` (used when the `local`
/// feature is active) is available, wiring up a connected client/server pair.
async fn with_pair<F, Fut>(
    server: MrtrServer,
    client_protocol: ProtocolVersion,
    body: F,
) -> anyhow::Result<()>
where
    F: FnOnce(rmcp::service::RunningService<RoleClient, MrtrClient>) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<()>>,
{
    tokio::task::LocalSet::new()
        .run_until(async move {
            let (server_transport, client_transport) = tokio::io::duplex(8192);
            let server_peer_info = client_info(client_protocol);
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
                MrtrClient,
                client_transport,
                Some(server_info_2026()),
            );

            let result = body(client).await;

            server_task.abort();
            result
        })
        .await
}

// =============================================================================
// Tests
// =============================================================================

#[tokio::test(flavor = "current_thread")]
async fn client_auto_fulfills_input_required_tool_call() -> anyhow::Result<()> {
    let server = MrtrServer::default();
    let calls = server.calls.clone();
    with_pair(server, ProtocolVersion::V_2026_07_28, |client| async move {
        let result = client
            .call_tool(CallToolRequestParams::new("single"))
            .await?;
        assert_eq!(result.content[0].as_text().unwrap().text, "done");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        Ok(())
    })
    .await
}

#[tokio::test(flavor = "current_thread")]
async fn manual_once_returns_input_required_without_retry() -> anyhow::Result<()> {
    let server = MrtrServer::default();
    let calls = server.calls.clone();
    with_pair(server, ProtocolVersion::V_2026_07_28, |client| async move {
        let result = client
            .call_tool_once(CallToolRequestParams::new("single"))
            .await?;
        assert!(matches!(result, CallToolResponse::InputRequired(_)));
        // A manual round makes exactly one server call and never retries.
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        Ok(())
    })
    .await
}

#[tokio::test(flavor = "current_thread")]
async fn multi_round_input_required_completes() -> anyhow::Result<()> {
    let server = MrtrServer::default();
    let calls = server.calls.clone();
    with_pair(server, ProtocolVersion::V_2026_07_28, |client| async move {
        let result = client
            .call_tool(CallToolRequestParams::new("multi_round"))
            .await?;
        assert_eq!(result.content[0].as_text().unwrap().text, "multi-done");
        // round 0 + two retries = 3 server calls.
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        Ok(())
    })
    .await
}

#[tokio::test(flavor = "current_thread")]
async fn multiple_input_requests_fulfilled_in_one_round() -> anyhow::Result<()> {
    let server = MrtrServer::default();
    let calls = server.calls.clone();
    with_pair(server, ProtocolVersion::V_2026_07_28, |client| async move {
        let result = client
            .call_tool(CallToolRequestParams::new("multi_request"))
            .await?;
        assert_eq!(result.content[0].as_text().unwrap().text, "multi-req-done");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        Ok(())
    })
    .await
}

#[tokio::test(flavor = "current_thread")]
async fn state_only_input_required_completes() -> anyhow::Result<()> {
    let server = MrtrServer::default();
    let calls = server.calls.clone();
    with_pair(server, ProtocolVersion::V_2026_07_28, |client| async move {
        let result = client
            .call_tool(CallToolRequestParams::new("state_only"))
            .await?;
        assert_eq!(result.content[0].as_text().unwrap().text, "state-done");
        // round 0 + two state-only retries = 3 server calls.
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        Ok(())
    })
    .await
}

#[tokio::test(flavor = "current_thread")]
async fn max_rounds_exceeded_returns_error() -> anyhow::Result<()> {
    let server = MrtrServer::default();
    let calls = server.calls.clone();
    with_pair(server, ProtocolVersion::V_2026_07_28, |client| async move {
        let err = client
            .call_tool_with_mrtr_max_rounds(CallToolRequestParams::new("loops"), 3)
            .await
            .expect_err("a tool that never completes must exhaust the round cap");
        assert!(matches!(
            err,
            ServiceError::InputRequiredRoundsExceeded { max_rounds: 3 }
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        Ok(())
    })
    .await
}

#[tokio::test(flavor = "current_thread")]
async fn client_handler_error_propagates() -> anyhow::Result<()> {
    let server = MrtrServer::default();
    with_pair(server, ProtocolVersion::V_2026_07_28, |client| async move {
        let err = client
            .call_tool(CallToolRequestParams::new("handler_error"))
            .await
            .expect_err("a failing input handler must fail the whole call");
        assert!(matches!(err, ServiceError::McpError(_)));
        Ok(())
    })
    .await
}

#[tokio::test(flavor = "current_thread")]
async fn request_state_is_echoed_byte_exact() -> anyhow::Result<()> {
    let server = MrtrServer::default();
    with_pair(server, ProtocolVersion::V_2026_07_28, |client| async move {
        // The server returns an error result unless it sees TRICKY_STATE echoed
        // back unchanged, so a successful completion proves the byte-exact echo.
        let result = client
            .call_tool(CallToolRequestParams::new("echo_state"))
            .await?;
        assert_eq!(result.content[0].as_text().unwrap().text, "echo-ok");
        Ok(())
    })
    .await
}

#[tokio::test(flavor = "current_thread")]
async fn get_prompt_auto_fulfills_input_required() -> anyhow::Result<()> {
    let server = MrtrServer::default();
    with_pair(server, ProtocolVersion::V_2026_07_28, |client| async move {
        let result = client.get_prompt(GetPromptRequestParams::new("p")).await?;
        assert_eq!(
            result.messages[0].content.as_text().unwrap().text,
            "prompt-done"
        );
        Ok(())
    })
    .await
}

#[tokio::test(flavor = "current_thread")]
async fn read_resource_auto_fulfills_input_required() -> anyhow::Result<()> {
    let server = MrtrServer::default();
    with_pair(server, ProtocolVersion::V_2026_07_28, |client| async move {
        let result = client
            .read_resource(ReadResourceRequestParams::new("res://x"))
            .await?;
        let text = match &result.contents[0] {
            ResourceContents::TextResourceContents { text, .. } => text.clone(),
            _ => panic!("expected text resource"),
        };
        assert_eq!(text, "resource-done");
        Ok(())
    })
    .await
}

#[tokio::test(flavor = "current_thread")]
async fn old_protocol_rejects_input_required() -> anyhow::Result<()> {
    let server = MrtrServer::default();
    // The client negotiated 2025-11-25, so the server must refuse to emit an
    // InputRequiredResult and return a protocol error instead.
    with_pair(server, ProtocolVersion::V_2025_11_25, |client| async move {
        let err = client
            .call_tool_once(CallToolRequestParams::new("single"))
            .await
            .expect_err("MRTR must be rejected for pre-2026 peers");
        match err {
            ServiceError::McpError(error) => {
                assert!(
                    error.message.contains("2026-07-28"),
                    "unexpected error message: {}",
                    error.message
                );
            }
            other => panic!("expected an McpError, got {other:?}"),
        }
        Ok(())
    })
    .await
}

#[cfg(feature = "request-state")]
#[tokio::test(flavor = "current_thread")]
async fn request_state_codec_seals_and_verifies_through_the_loop() -> anyhow::Result<()> {
    use std::sync::OnceLock;

    use rmcp::model::RequestStateCodec;

    // A shared per-process signing key, mirroring how a real server would derive one.
    static KEY: &[u8] = b"integration-signing-key-32-bytes!";

    fn codec() -> &'static RequestStateCodec {
        static CODEC: OnceLock<RequestStateCodec> = OnceLock::new();
        CODEC.get_or_init(|| RequestStateCodec::new(KEY))
    }

    #[derive(Clone, Default)]
    struct SealingServer;

    impl ServerHandler for SealingServer {
        fn get_info(&self) -> ServerInfo {
            let mut info = ServerInfo::new(ServerCapabilities::builder().enable_tools().build());
            info.protocol_version = ProtocolVersion::V_2026_07_28;
            info
        }

        async fn call_tool(
            &self,
            request: CallToolRequestParams,
            _context: RequestContext<RoleServer>,
        ) -> Result<CallToolResponse, ErrorData> {
            match request.request_state {
                None => {
                    let sealed = codec()
                        .seal_json(&json!({ "step": 1, "tool": request.name }))
                        .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
                    let mut requests = InputRequests::new();
                    requests.insert("answer".to_string(), elicitation_request("Name?"));
                    Ok(InputRequiredResult::new(Some(requests), Some(sealed)).into())
                }
                Some(sealed) => {
                    // The echoed state is untrusted; verify it before use.
                    let state: serde_json::Value = codec()
                        .open_json(&sealed)
                        .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
                    assert_eq!(state["step"], 1);
                    Ok(CallToolResult::success(vec![ContentBlock::text("sealed-done")]).into())
                }
            }
        }
    }

    tokio::task::LocalSet::new()
        .run_until(async move {
            let (server_transport, client_transport) = tokio::io::duplex(8192);
            let server_task = tokio::task::spawn_local(async move {
                let running = serve_directly::<RoleServer, _, _, _, _>(
                    SealingServer,
                    server_transport,
                    Some(client_info(ProtocolVersion::V_2026_07_28)),
                );
                running.waiting().await?;
                anyhow::Ok(())
            });

            let client = serve_directly::<RoleClient, _, _, _, _>(
                MrtrClient,
                client_transport,
                Some(server_info_2026()),
            );

            let result = client
                .call_tool(CallToolRequestParams::new("sealed"))
                .await?;
            assert_eq!(result.content[0].as_text().unwrap().text, "sealed-done");

            server_task.abort();
            anyhow::Ok(())
        })
        .await
}
