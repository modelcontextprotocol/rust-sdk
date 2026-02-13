//! Tests for task support validation in tool calls.
//!
//! Verifies that the server correctly validates `execution.taskSupport` settings
//! per the MCP specification:
//! - `Required`: MUST be invoked as a task, returns -32601 otherwise
//! - `Forbidden`: MUST NOT be invoked as a task, returns error otherwise
//! - `Optional`: MAY be invoked either way

use rmcp::{
    ClientHandler, ServerHandler, ServiceError, ServiceExt,
    handler::server::router::tool::ToolRouter,
    model::{CallToolRequestParams, ClientInfo, ErrorCode, JsonObject},
    tool, tool_handler, tool_router,
};
use serde_json::json;

/// Server with tools having different task support modes.
#[derive(Debug, Clone)]
pub struct TaskSupportTestServer {
    tool_router: ToolRouter<Self>,
}

impl Default for TaskSupportTestServer {
    fn default() -> Self {
        Self::new()
    }
}

impl TaskSupportTestServer {
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }
}

#[tool_router]
impl TaskSupportTestServer {
    #[tool(
        description = "Tool that requires task-based invocation",
        execution(task_support = "required")
    )]
    async fn required_task_tool(&self) -> String {
        "required task executed".to_string()
    }

    #[tool(
        description = "Tool that forbids task-based invocation",
        execution(task_support = "forbidden")
    )]
    async fn forbidden_task_tool(&self) -> String {
        "forbidden task executed".to_string()
    }

    #[tool(
        description = "Tool that optionally supports task-based invocation",
        execution(task_support = "optional")
    )]
    async fn optional_task_tool(&self) -> String {
        "optional task executed".to_string()
    }
}

#[tool_handler]
impl ServerHandler for TaskSupportTestServer {}

#[derive(Debug, Clone, Default)]
struct DummyClientHandler {}

impl ClientHandler for DummyClientHandler {
    fn get_info(&self) -> ClientInfo {
        ClientInfo::default()
    }
}

/// Helper to create a task object for tool calls
fn make_task() -> Option<JsonObject> {
    Some(json!({}).as_object().unwrap().clone())
}

#[tokio::test]
async fn test_required_task_tool_without_task_returns_method_not_found() -> anyhow::Result<()> {
    let (server_transport, client_transport) = tokio::io::duplex(4096);

    let server = TaskSupportTestServer::new();
    let server_handle = tokio::spawn(async move {
        server.serve(server_transport).await?.waiting().await?;
        anyhow::Ok(())
    });

    let client_handler = DummyClientHandler::default();
    let client = client_handler.serve(client_transport).await?;

    // Call the task-required tool without a task - should fail with -32601
    let result = client
        .call_tool(CallToolRequestParams {
            meta: None,
            name: "required_task_tool".into(),
            arguments: None,
            task: None, // No task provided!
        })
        .await;

    // Should be an error with code -32601 (METHOD_NOT_FOUND)
    assert!(
        result.is_err(),
        "Expected error for required task tool without task"
    );
    let error = result.unwrap_err();

    // Check the error data contains the expected code
    match error {
        ServiceError::McpError(error_data) => {
            assert_eq!(
                error_data.code,
                ErrorCode::METHOD_NOT_FOUND,
                "Expected METHOD_NOT_FOUND error code (-32601)"
            );
            assert!(
                error_data
                    .message
                    .contains("requires task-based invocation"),
                "Error message should indicate task-based invocation is required, got: {}",
                error_data.message
            );
        }
        _ => panic!("Expected McpError variant, got: {:?}", error),
    }

    client.cancel().await?;
    server_handle.await??;
    Ok(())
}

#[tokio::test]
async fn test_forbidden_task_tool_with_task_returns_error() -> anyhow::Result<()> {
    let (server_transport, client_transport) = tokio::io::duplex(4096);

    let server = TaskSupportTestServer::new();
    let server_handle = tokio::spawn(async move {
        server.serve(server_transport).await?.waiting().await?;
        anyhow::Ok(())
    });

    let client_handler = DummyClientHandler::default();
    let client = client_handler.serve(client_transport).await?;

    // Call the forbidden task tool WITH a task - should fail
    let result = client
        .call_tool(CallToolRequestParams {
            meta: None,
            name: "forbidden_task_tool".into(),
            arguments: None,
            task: make_task(), // Task provided but forbidden!
        })
        .await;

    // Should be an error with code INVALID_PARAMS
    assert!(
        result.is_err(),
        "Expected error for forbidden task tool with task"
    );
    let error = result.unwrap_err();

    // Check the error data contains the expected code
    match error {
        ServiceError::McpError(error_data) => {
            assert_eq!(
                error_data.code,
                ErrorCode::INVALID_PARAMS,
                "Expected INVALID_PARAMS error code"
            );
            assert!(
                error_data
                    .message
                    .contains("does not support task-based invocation"),
                "Error message should indicate task-based invocation is not supported, got: {}",
                error_data.message
            );
        }
        _ => panic!("Expected McpError variant, got: {:?}", error),
    }

    client.cancel().await?;
    server_handle.await??;
    Ok(())
}

#[tokio::test]
async fn test_forbidden_task_tool_without_task_succeeds() -> anyhow::Result<()> {
    let (server_transport, client_transport) = tokio::io::duplex(4096);

    let server = TaskSupportTestServer::new();
    let server_handle = tokio::spawn(async move {
        server.serve(server_transport).await?.waiting().await?;
        anyhow::Ok(())
    });

    let client_handler = DummyClientHandler::default();
    let client = client_handler.serve(client_transport).await?;

    // Call the forbidden task tool WITHOUT a task - should succeed
    let result = client
        .call_tool(CallToolRequestParams {
            meta: None,
            name: "forbidden_task_tool".into(),
            arguments: None,
            task: None, // No task - allowed for forbidden
        })
        .await;

    assert!(
        result.is_ok(),
        "Forbidden task tool without task should succeed"
    );
    let result = result.unwrap();
    let text = result
        .content
        .first()
        .and_then(|c| c.raw.as_text())
        .map(|t| t.text.as_str())
        .unwrap_or("");
    assert_eq!(text, "forbidden task executed");

    client.cancel().await?;
    server_handle.await??;
    Ok(())
}

#[tokio::test]
async fn test_optional_task_tool_without_task_succeeds() -> anyhow::Result<()> {
    let (server_transport, client_transport) = tokio::io::duplex(4096);

    let server = TaskSupportTestServer::new();
    let server_handle = tokio::spawn(async move {
        server.serve(server_transport).await?.waiting().await?;
        anyhow::Ok(())
    });

    let client_handler = DummyClientHandler::default();
    let client = client_handler.serve(client_transport).await?;

    // Call the optional task tool WITHOUT a task - should succeed
    let result = client
        .call_tool(CallToolRequestParams {
            meta: None,
            name: "optional_task_tool".into(),
            arguments: None,
            task: None, // No task - allowed for optional
        })
        .await;

    assert!(
        result.is_ok(),
        "Optional task tool without task should succeed"
    );
    let result = result.unwrap();
    let text = result
        .content
        .first()
        .and_then(|c| c.raw.as_text())
        .map(|t| t.text.as_str())
        .unwrap_or("");
    assert_eq!(text, "optional task executed");

    client.cancel().await?;
    server_handle.await??;
    Ok(())
}
