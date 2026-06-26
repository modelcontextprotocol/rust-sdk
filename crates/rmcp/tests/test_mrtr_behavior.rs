use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use rmcp::{
    ClientHandler, ServerHandler,
    model::*,
    service::{RequestContext, RoleClient, RoleServer, serve_directly},
};
use serde_json::json;

#[derive(Clone, Default)]
struct MrtrServer {
    calls: Arc<AtomicUsize>,
}

impl ServerHandler for MrtrServer {
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
        self.calls.fetch_add(1, Ordering::SeqCst);

        if let Some(input_responses) = request.input_responses {
            assert_eq!(request.request_state.as_deref(), Some("opaque-state"));
            let answer = input_responses
                .get("answer")
                .expect("answer input response should be echoed");
            assert_eq!(answer["action"], "accept");
            assert_eq!(answer["content"]["name"], "Ferris");
            return Ok(CallToolResult::success(vec![ContentBlock::text("done")]).into());
        }

        let mut input_requests = InputRequests::new();
        input_requests.insert(
            "answer".to_string(),
            InputRequest::Elicitation(ElicitRequest::new(
                ElicitRequestParams::FormElicitationParams {
                    meta: None,
                    message: "Name?".into(),
                    requested_schema: serde_json::from_value(json!({
                        "type": "object",
                        "properties": {
                            "name": { "type": "string" }
                        },
                        "required": ["name"]
                    }))
                    .unwrap(),
                },
            )),
        );
        Ok(InputRequiredResult::new(Some(input_requests), Some("opaque-state".into())).into())
    }
}

#[derive(Clone, Default)]
struct MrtrClient;

impl ClientHandler for MrtrClient {
    async fn create_elicitation(
        &self,
        _request: ElicitRequestParams,
        _context: RequestContext<RoleClient>,
    ) -> Result<ElicitResult, ErrorData> {
        Ok(
            ElicitResult::new(ElicitationAction::Accept).with_content(json!({
                "name": "Ferris"
            })),
        )
    }
}

fn client_info_2026() -> ClientInfo {
    ClientInfo::new(
        ClientCapabilities::builder().enable_elicitation().build(),
        Implementation::new("mrtr-test-client", "0.0.0"),
    )
    .with_protocol_version(ProtocolVersion::V_2026_07_28)
}

fn server_info_2026() -> ServerInfo {
    let mut info = ServerInfo::new(ServerCapabilities::builder().enable_tools().build());
    info.protocol_version = ProtocolVersion::V_2026_07_28;
    info
}

#[tokio::test(flavor = "current_thread")]
async fn client_auto_fulfills_input_required_tool_call() -> anyhow::Result<()> {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (server_transport, client_transport) = tokio::io::duplex(8192);
            let server = MrtrServer::default();
            let calls = server.calls.clone();

            let server_task = tokio::task::spawn_local(async move {
                let running = serve_directly::<RoleServer, _, _, _, _>(
                    server,
                    server_transport,
                    Some(client_info_2026()),
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
                .call_tool(CallToolRequestParams::new("needs_input"))
                .await?;
            assert_eq!(result.content.len(), 1);
            assert_eq!(result.content[0].as_text().unwrap().text, "done");
            assert_eq!(calls.load(Ordering::SeqCst), 2);

            drop(client);
            server_task.abort();
            Ok(())
        })
        .await
}

#[tokio::test(flavor = "current_thread")]
async fn manual_once_returns_input_required_without_retry() -> anyhow::Result<()> {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (server_transport, client_transport) = tokio::io::duplex(8192);
            let server = MrtrServer::default();

            let server_task = tokio::task::spawn_local(async move {
                let running = serve_directly::<RoleServer, _, _, _, _>(
                    server,
                    server_transport,
                    Some(client_info_2026()),
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
                .call_tool_once(CallToolRequestParams::new("needs_input"))
                .await?;
            assert!(matches!(result, CallToolResponse::InputRequired(_)));

            drop(client);
            server_task.abort();
            Ok(())
        })
        .await
}
