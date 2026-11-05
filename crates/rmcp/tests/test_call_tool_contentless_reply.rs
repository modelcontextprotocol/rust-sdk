//! A `tools/call` reply that omits `content` decodes as a non-tool
//! `ServerResult` variant; `call_tool` still returns it as an empty result.

use rmcp::{
    model::{CallToolRequestParams, CallToolResult, InitializeResult},
    service::{RoleClient, ServiceError, serve_directly},
};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// Calls a tool against a raw peer that answers the request with `reply`.
async fn call_tool_with_reply(reply: Value) -> Result<CallToolResult, ServiceError> {
    tokio::task::LocalSet::new()
        .run_until(async move {
            let (client_transport, server_transport) = tokio::io::duplex(4096);
            let (read, mut write) = tokio::io::split(server_transport);
            let client = serve_directly::<RoleClient, _, _, _, _>(
                (),
                client_transport,
                Some(InitializeResult::default().into()),
            );

            let respond = async {
                let mut lines = BufReader::new(read).lines();
                let line = lines.next_line().await.unwrap().unwrap();
                let request: Value = serde_json::from_str(&line).unwrap();
                let response = json!({ "jsonrpc": "2.0", "id": request["id"], "result": reply });
                write
                    .write_all(format!("{response}\n").as_bytes())
                    .await
                    .unwrap();
                // Keep the transport open until the client has read the reply.
                (lines, write)
            };
            let (_peer, result) = tokio::join!(
                respond,
                client.call_tool(CallToolRequestParams::new("probe"))
            );
            result
        })
        .await
}

fn call_tool_result(value: Value) -> CallToolResult {
    serde_json::from_value(value).unwrap()
}

#[tokio::test]
async fn call_tool_should_accept_meta_only_reply() {
    let result = call_tool_with_reply(json!({ "_meta": { "example.com/replica": "r1" } })).await;

    assert_eq!(
        result.unwrap(),
        call_tool_result(json!({ "content": [], "_meta": { "example.com/replica": "r1" } }))
    );
}

#[tokio::test]
async fn call_tool_should_accept_result_type_and_meta_reply() {
    let result = call_tool_with_reply(json!({
        "resultType": "complete",
        "_meta": { "example.com/replica": "r1" }
    }))
    .await;

    assert_eq!(
        result.unwrap(),
        call_tool_result(json!({
            "resultType": "complete",
            "content": [],
            "_meta": { "example.com/replica": "r1" }
        }))
    );
}

#[tokio::test]
async fn call_tool_should_accept_empty_reply() {
    let result = call_tool_with_reply(json!({})).await;

    assert_eq!(result.unwrap(), call_tool_result(json!({ "content": [] })));
}

#[tokio::test]
async fn call_tool_should_reject_unrelated_reply() {
    let result = call_tool_with_reply(json!({ "skills": [], "_meta": {} })).await;

    assert!(matches!(result, Err(ServiceError::UnexpectedResponse)));
}
