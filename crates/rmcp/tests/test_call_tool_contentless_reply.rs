//! A `tools/call` reply that omits `content` decodes as a non-tool
//! `ServerResult` variant; `call_tool` still returns it as an empty result.

use rmcp::{
    model::{CallToolRequestParams, CallToolResult, InitializeResult},
    service::{RoleClient, ServiceError, serve_directly},
};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// Calls a tool through `RunningService::call_tool` and `Peer::call_tool`
/// against a raw peer that answers each request with `reply`.
async fn call_tool_with_reply(reply: Value) -> [Result<CallToolResult, ServiceError>; 2] {
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
                for _ in 0..2 {
                    let line = lines.next_line().await.unwrap().unwrap();
                    let request: Value = serde_json::from_str(&line).unwrap();
                    let response =
                        json!({ "jsonrpc": "2.0", "id": request["id"], "result": reply });
                    write
                        .write_all(format!("{response}\n").as_bytes())
                        .await
                        .unwrap();
                }
                // Keep the transport open until the client has read the replies.
                (lines, write)
            };
            let call = async {
                let params = CallToolRequestParams::new("probe");
                [
                    client.call_tool(params.clone()).await,
                    client.peer().call_tool(params).await,
                ]
            };
            let (_peer, results) = tokio::join!(respond, call);
            results
        })
        .await
}

fn call_tool_result(value: Value) -> CallToolResult {
    serde_json::from_value(value).unwrap()
}

#[tokio::test]
async fn call_tool_should_accept_meta_only_reply() {
    let expected =
        call_tool_result(json!({ "content": [], "_meta": { "example.com/replica": "r1" } }));

    for result in call_tool_with_reply(json!({ "_meta": { "example.com/replica": "r1" } })).await {
        assert_eq!(result.unwrap(), expected);
    }
}

#[tokio::test]
async fn call_tool_should_accept_result_type_and_meta_reply() {
    let expected = call_tool_result(json!({
        "resultType": "complete",
        "content": [],
        "_meta": { "example.com/replica": "r1" }
    }));

    for result in call_tool_with_reply(json!({
        "resultType": "complete",
        "_meta": { "example.com/replica": "r1" }
    }))
    .await
    {
        assert_eq!(result.unwrap(), expected);
    }
}

#[tokio::test]
async fn call_tool_should_accept_empty_reply() {
    let expected = call_tool_result(json!({ "content": [] }));

    for result in call_tool_with_reply(json!({})).await {
        assert_eq!(result.unwrap(), expected);
    }
}

#[tokio::test]
async fn call_tool_should_reject_unrelated_reply() {
    for result in call_tool_with_reply(json!({ "skills": [], "_meta": {} })).await {
        assert!(matches!(result, Err(ServiceError::UnexpectedResponse)));
    }
}
