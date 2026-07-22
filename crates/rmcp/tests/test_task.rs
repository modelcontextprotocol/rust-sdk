//! End-to-end tests for the MCP Tasks extension (SEP-2663,
//! `io.modelcontextprotocol/tasks`).
#![cfg(all(feature = "server", feature = "client", not(feature = "local")))]

use rmcp::{
    ErrorData as McpError, ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::*,
    service::{RequestContext, RoleServer},
    task_manager::{TaskExit, TaskManager, TaskOptions},
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
            .client_capabilities()
            .is_some_and(|caps| caps.supports_tasks());

        if request.name == "sum" && client_supports_tasks {
            let args: SumArgs = serde_json::from_value(serde_json::Value::Object(
                request.arguments.clone().unwrap_or_default(),
            ))
            .map_err(|e| McpError::invalid_params(e.to_string(), None))?;
            let task = self
                .tasks
                .spawn(TaskOptions::new().with_poll_interval_ms(10), move |ctx| {
                    Box::pin(async move {
                        tokio::select! {
                            _ = ctx.cancelled() => {
                                Err(TaskExit::Cancelled)
                            }
                            _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => {
                                Ok(CallToolResult::success(vec![ContentBlock::text(
                                    (args.a + args.b).to_string(),
                                )]))
                            }
                        }
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

    // Cancellation is cooperative (SEP-2663): the ack is immediate, and the
    // operation settles the terminal status; poll until it does.
    let mut final_status = None;
    for _ in 0..100 {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        let info = client
            .peer()
            .get_task(GetTaskParams::new(create.task.task_id.clone()))
            .await
            .unwrap();
        if info.task.status().is_terminal() {
            final_status = Some(info.task.status());
            break;
        }
    }
    assert_eq!(final_status, Some(TaskStatus::Cancelled));

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

/// A misbehaving handler that materializes a task without checking the
/// client's capabilities. The SDK dispatch must catch this and reject with
/// -32021 rather than sending a task handle the client cannot parse.
#[derive(Clone)]
struct AlwaysTaskServer {
    tasks: TaskManager,
}

impl ServerHandler for AlwaysTaskServer {
    async fn call_tool(
        &self,
        _request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let task = self.tasks.spawn(TaskOptions::default(), |_ctx| {
            Box::pin(async { Ok(CallToolResult::success(vec![ContentBlock::text("late")])) })
        });
        Ok(CallToolResponse::Task(CreateTaskResult::new(task)))
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

#[tokio::test]
async fn dispatch_rejects_task_result_for_non_declaring_client() {
    let (server_transport, client_transport) = tokio::io::duplex(4096);
    let server = tokio::spawn(async move {
        let service = AlwaysTaskServer {
            tasks: TaskManager::new(),
        }
        .serve(server_transport)
        .await?;
        service.waiting().await?;
        anyhow::Ok(())
    });

    // Plain client: no tasks extension declared. The handler tries to return
    // a CreateTaskResult anyway; dispatch must reject with -32021.
    let client = ().serve(client_transport).await.unwrap();
    let err = client
        .call_tool(CallToolRequestParams::new("anything"))
        .await
        .unwrap_err();
    match err {
        rmcp::ServiceError::McpError(e) => {
            assert_eq!(e.code, ErrorCode::MISSING_REQUIRED_CLIENT_CAPABILITY);
        }
        other => panic!("expected McpError, got {other:?}"),
    }

    client.cancel().await.unwrap();
    server.abort();
}

#[tokio::test]
async fn tasks_methods_without_capability_return_missing_capability_error() {
    let (server_transport, client_transport) = tokio::io::duplex(4096);
    let server = tokio::spawn(async move {
        let service = TaskServer::new().serve(server_transport).await?;
        service.waiting().await?;
        anyhow::Ok(())
    });

    // Plain client: no tasks extension declared. tasks/* must be rejected
    // with -32021 Missing Required Client Capability (SEP-2663), not -32601.
    let client = ().serve(client_transport).await.unwrap();
    let err = client
        .peer()
        .get_task(GetTaskParams::new("whatever"))
        .await
        .unwrap_err();
    match err {
        rmcp::ServiceError::McpError(e) => {
            assert_eq!(e.code, ErrorCode::MISSING_REQUIRED_CLIENT_CAPABILITY);
            let data = e.data.expect("error data should be present");
            assert!(
                data["requiredCapabilities"]["extensions"]
                    .as_object()
                    .is_some_and(|ext| ext.contains_key("io.modelcontextprotocol/tasks")),
                "error data should name the tasks extension: {data}"
            );
        }
        other => panic!("expected McpError, got {other:?}"),
    }

    let err = client
        .peer()
        .cancel_task(CancelTaskParams::new("whatever"))
        .await
        .unwrap_err();
    match err {
        rmcp::ServiceError::McpError(e) => {
            assert_eq!(e.code, ErrorCode::MISSING_REQUIRED_CLIENT_CAPABILITY);
        }
        other => panic!("expected McpError, got {other:?}"),
    }

    client.cancel().await.unwrap();
    server.abort();
}

#[tokio::test]
async fn unknown_task_id_returns_invalid_params() {
    let (server_transport, client_transport) = tokio::io::duplex(4096);
    let server = tokio::spawn(async move {
        let service = TaskServer::new().serve(server_transport).await?;
        service.waiting().await?;
        anyhow::Ok(())
    });

    let client = tasks_client_info().serve(client_transport).await.unwrap();
    let err = client
        .peer()
        .get_task(GetTaskParams::new("no-such-task"))
        .await
        .unwrap_err();
    match err {
        rmcp::ServiceError::McpError(e) => {
            // SEP-2663: unknown taskId is -32602 Invalid params.
            assert_eq!(e.code, ErrorCode::INVALID_PARAMS);
        }
        other => panic!("expected McpError, got {other:?}"),
    }

    client.cancel().await.unwrap();
    server.abort();
}

#[tokio::test]
async fn legacy_task_param_is_ignored() {
    let (server_transport, client_transport) = tokio::io::duplex(4096);
    let server = tokio::spawn(async move {
        let service = TaskServer::new().serve(server_transport).await?;
        service.waiting().await?;
        anyhow::Ok(())
    });

    // Plain client sending a legacy 2025-11-25 `task: {...}` param: it must
    // be silently ignored and the call answered synchronously (SEP-2663).
    let client = ().serve(client_transport).await.unwrap();
    let params: CallToolRequestParams = serde_json::from_value(json!({
        "name": "sum",
        "arguments": {"a": 2, "b": 3},
        "task": {"ttl": 60000}
    }))
    .expect("legacy task param must not break deserialization");
    let result = client.call_tool(params).await.unwrap();
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
