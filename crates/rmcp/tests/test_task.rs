//! End-to-end tests for the MCP Tasks extension (SEP-2663,
//! `io.modelcontextprotocol/tasks`).
#![cfg(all(feature = "server", feature = "client", not(feature = "local")))]

use rmcp::{
    ErrorData as McpError, ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::*,
    service::{RequestContext, RoleServer},
    task_manager::{TaskManager, TaskOptions},
    tool, tool_router,
};
use serde_json::json;

#[derive(Debug, serde::Deserialize, rmcp::schemars::JsonSchema)]
pub struct SumArgs {
    pub a: i32,
    pub b: i32,
}

#[derive(Clone)]
struct TaskServer {
    tool_router: ToolRouter<TaskServer>,
    tasks: TaskManager,
}

#[tool_router]
impl TaskServer {
    fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
            tasks: TaskManager::new(),
        }
    }

    #[tool(description = "Sum two numbers")]
    async fn sum(
        &self,
        Parameters(SumArgs { a, b }): Parameters<SumArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(CallToolResult::success(vec![ContentBlock::text(
            (a + b).to_string(),
        )]))
    }
}

impl ServerHandler for TaskServer {
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let client_supports_tasks = context
            .meta
            .client_capabilities()
            .map(|caps| caps.supports_tasks())
            .unwrap_or_else(|| {
                context
                    .peer
                    .peer_info()
                    .is_some_and(|info| info.capabilities.supports_tasks())
            });

        if request.name == "sum" && client_supports_tasks {
            let args: SumArgs = serde_json::from_value(serde_json::Value::Object(
                request.arguments.clone().unwrap_or_default(),
            ))
            .map_err(|e| McpError::invalid_params(e.to_string(), None))?;
            let task =
                self.tasks
                    .spawn(TaskOptions::new().with_poll_interval_ms(10), move |_ctx| {
                        Box::pin(async move {
                            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                            Ok(CallToolResult::success(vec![ContentBlock::text(
                                (args.a + args.b).to_string(),
                            )]))
                        })
                    });
            return Ok(CallToolResponse::Task(CreateTaskResult::new(task)));
        }

        let tcc = rmcp::handler::server::tool::ToolCallContext::new(self, request, context);
        self.tool_router.call(tcc).await
    }

    async fn get_task(
        &self,
        request: GetTaskParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<GetTaskResult, McpError> {
        Ok(GetTaskResult::new(self.tasks.get_task(&request.task_id)?))
    }

    async fn update_task(
        &self,
        request: UpdateTaskParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<(), McpError> {
        self.tasks
            .update_task(&request.task_id, request.input_responses)
    }

    async fn cancel_task(
        &self,
        request: CancelTaskParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<(), McpError> {
        self.tasks.cancel_task(&request.task_id)
    }

    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_tasks()
                .build(),
        )
    }
}

fn tasks_client_info() -> ClientInfo {
    ClientInfo::new(
        ClientCapabilities::builder().enable_tasks().build(),
        Implementation::from_build_env(),
    )
}

#[tokio::test]
async fn task_lifecycle_create_poll_complete() {
    let (server_transport, client_transport) = tokio::io::duplex(4096);
    let server = tokio::spawn(async move {
        let service = TaskServer::new().serve(server_transport).await?;
        service.waiting().await?;
        anyhow::Ok(())
    });

    let client = tasks_client_info().serve(client_transport).await.unwrap();

    // Server materializes a task because we declared the extension.
    let response = client
        .call_tool_once(
            CallToolRequestParams::new("sum")
                .with_arguments(serde_json::from_value(json!({"a": 40, "b": 2})).unwrap()),
        )
        .await
        .unwrap();
    let create = match response {
        CallToolResponse::Task(create) => create,
        other => panic!("expected CreateTaskResult, got {other:?}"),
    };
    assert_eq!(create.result_type, ResultType::TASK);
    let task_id = create.task.task_id.clone();

    // Poll until terminal.
    let final_task = loop {
        tokio::time::sleep(std::time::Duration::from_millis(
            create.task.poll_interval_ms.unwrap_or(10),
        ))
        .await;
        let info = client
            .peer()
            .get_task(GetTaskParams::new(task_id.clone()))
            .await
            .unwrap();
        if info.task.status().is_terminal() {
            break info.task;
        }
    };

    match final_task.payload {
        TaskPayload::Completed { result } => {
            let result: CallToolResult =
                serde_json::from_value(serde_json::Value::Object(result)).unwrap();
            let text = result.content[0].as_text().unwrap();
            assert_eq!(text.text, "42");
        }
        other => panic!("expected completed task, got {other:?}"),
    }

    client.cancel().await.unwrap();
    server.abort();
}

#[tokio::test]
async fn task_cancel_acknowledged() {
    let (server_transport, client_transport) = tokio::io::duplex(4096);
    let server = tokio::spawn(async move {
        let service = TaskServer::new().serve(server_transport).await?;
        service.waiting().await?;
        anyhow::Ok(())
    });

    let client = tasks_client_info().serve(client_transport).await.unwrap();

    let response = client
        .call_tool_once(
            CallToolRequestParams::new("sum")
                .with_arguments(serde_json::from_value(json!({"a": 1, "b": 1})).unwrap()),
        )
        .await
        .unwrap();
    let create = match response {
        CallToolResponse::Task(create) => create,
        other => panic!("expected CreateTaskResult, got {other:?}"),
    };

    client
        .peer()
        .cancel_task(CancelTaskParams::new(create.task.task_id.clone()))
        .await
        .unwrap();

    let info = client
        .peer()
        .get_task(GetTaskParams::new(create.task.task_id.clone()))
        .await
        .unwrap();
    assert_eq!(info.task.status(), TaskStatus::Cancelled);

    client.cancel().await.unwrap();
    server.abort();
}

#[tokio::test]
async fn no_task_without_extension_capability() {
    let (server_transport, client_transport) = tokio::io::duplex(4096);
    let server = tokio::spawn(async move {
        let service = TaskServer::new().serve(server_transport).await?;
        service.waiting().await?;
        anyhow::Ok(())
    });

    // Plain client: no tasks extension declared.
    let client = ().serve(client_transport).await.unwrap();
    let result = client
        .call_tool(
            CallToolRequestParams::new("sum")
                .with_arguments(serde_json::from_value(json!({"a": 2, "b": 3})).unwrap()),
        )
        .await
        .unwrap();
    let text = result.content[0].as_text().unwrap();
    assert_eq!(text.text, "5");

    client.cancel().await.unwrap();
    server.abort();
}

#[test]
fn task_status_notification_params_preserve_meta() {
    let raw = json!({
        "_meta": {
            "traceId": "trace-1"
        },
        "taskId": "task-1",
        "status": "working",
        "createdAt": "2026-06-24T00:00:00Z",
        "lastUpdatedAt": "2026-06-24T00:00:01Z",
        "ttlMs": null
    });

    let params: TaskStatusNotificationParams = serde_json::from_value(raw).unwrap();

    assert_eq!(params.task.task.task_id, "task-1");
    assert_eq!(params.status(), TaskStatus::Working);
    assert_eq!(params.meta.as_ref().unwrap().0["traceId"], json!("trace-1"));

    let serialized = serde_json::to_value(&params).unwrap();
    assert_eq!(serialized["_meta"]["traceId"], json!("trace-1"));
    assert_eq!(serialized["taskId"], json!("task-1"));
    assert_eq!(serialized["ttlMs"], serde_json::Value::Null);
}
