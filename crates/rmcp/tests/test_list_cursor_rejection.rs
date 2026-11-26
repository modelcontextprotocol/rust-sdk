#![cfg(not(feature = "local"))]
#![cfg(all(feature = "client", feature = "server", feature = "macros"))]

use rmcp::{
    RoleClient, RoleServer, ServerHandler, Service, ServiceError, ServiceExt,
    handler::server::router::{Router, prompt::PromptRouter, tool::ToolRouter},
    model::{ErrorCode, PaginatedRequestParams},
    prompt_handler,
    service::RunningService,
    tool_handler,
};

#[derive(Debug, Clone)]
struct MacroServer {
    tool_router: ToolRouter<Self>,
    prompt_router: PromptRouter<Self>,
}

impl MacroServer {
    fn new() -> Self {
        Self {
            tool_router: ToolRouter::new(),
            prompt_router: PromptRouter::new(),
        }
    }
}

#[tool_handler(router = self.tool_router)]
#[prompt_handler(router = self.prompt_router)]
impl ServerHandler for MacroServer {}

#[derive(Debug, Clone)]
struct PlainServer;

impl ServerHandler for PlainServer {}

async fn connect<S: Service<RoleServer>>(server: S) -> RunningService<RoleClient, ()> {
    let (server_transport, client_transport) = tokio::io::duplex(4096);
    tokio::spawn(async move {
        let server = server.serve(server_transport).await.expect("server");
        server.waiting().await.expect("server task");
    });
    ().serve(client_transport).await.expect("client")
}

fn unissued_cursor() -> Option<PaginatedRequestParams> {
    Some(PaginatedRequestParams::default().with_cursor(Some("not-issued-by-this-server".into())))
}

fn error_code<T: std::fmt::Debug>(result: Result<T, ServiceError>) -> ErrorCode {
    match result {
        Err(ServiceError::McpError(error)) => error.code,
        other => panic!("expected an MCP error, got {other:?}"),
    }
}

#[tokio::test]
async fn tool_handler_should_reject_a_cursor_it_never_issued() {
    let client = connect(MacroServer::new()).await;

    let result = client.list_tools(unissued_cursor()).await;

    assert_eq!(error_code(result), ErrorCode::INVALID_PARAMS);
}

#[tokio::test]
async fn prompt_handler_should_reject_a_cursor_it_never_issued() {
    let client = connect(MacroServer::new()).await;

    let result = client.list_prompts(unissued_cursor()).await;

    assert_eq!(error_code(result), ErrorCode::INVALID_PARAMS);
}

#[tokio::test]
async fn router_should_reject_a_tools_cursor_it_never_issued() {
    let client = connect(Router::new(PlainServer)).await;

    let result = client.list_tools(unissued_cursor()).await;

    assert_eq!(error_code(result), ErrorCode::INVALID_PARAMS);
}

#[tokio::test]
async fn router_should_reject_a_prompts_cursor_it_never_issued() {
    let client = connect(Router::new(PlainServer)).await;

    let result = client.list_prompts(unissued_cursor()).await;

    assert_eq!(error_code(result), ErrorCode::INVALID_PARAMS);
}

#[tokio::test]
async fn tool_handler_should_list_tools_when_no_cursor_is_sent() {
    let client = connect(MacroServer::new()).await;

    let result = client.list_tools(None).await.expect("tools/list");

    assert_eq!(result.next_cursor, None);
}

#[tokio::test]
async fn prompt_handler_should_list_prompts_when_no_cursor_is_sent() {
    let client = connect(MacroServer::new()).await;

    let result = client.list_prompts(None).await.expect("prompts/list");

    assert_eq!(result.next_cursor, None);
}
