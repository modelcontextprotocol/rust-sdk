//! Skills methods must stay local until the server declares the extension.
#![cfg(not(feature = "local"))]
#![cfg(all(feature = "client", feature = "server"))]

use rmcp::{
    RoleClient,
    model::{
        ErrorCode, GetSkillRequestParams, InitializeResult, ServerCapabilities, ServerPeerInfo,
    },
    service::{ServiceError, serve_directly},
};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

async fn assert_no_skills_requests(peer_info: Option<ServerPeerInfo>) {
    let (client_transport, raw_transport) = tokio::io::duplex(4096);
    // Use a raw peer instead of a ServerHandler: the SDK's server-side
    // capability check would otherwise hide requests sent by the client.
    let raw_peer = tokio::spawn(async move {
        let (read, mut write) = tokio::io::split(raw_transport);
        let mut lines = BufReader::new(read).lines();
        let mut requests = Vec::new();
        while let Some(line) = lines.next_line().await.expect("read request") {
            let request: Value = serde_json::from_str(&line).expect("JSON request");
            requests.push(
                request["method"]
                    .as_str()
                    .expect("request method")
                    .to_owned(),
            );
            let response = json!({
                "jsonrpc": "2.0",
                "id": request["id"],
                "error": { "code": -32601, "message": "Method not found" },
            });
            let mut bytes = serde_json::to_vec(&response).expect("JSON response");
            bytes.push(b'\n');
            write.write_all(&bytes).await.expect("write response");
        }
        requests
    });
    let client = serve_directly::<RoleClient, _, _, _, _>((), client_transport, peer_info);

    let errors = [
        client
            .list_skills(None)
            .await
            .expect_err("skills undeclared"),
        client
            .get_skill(GetSkillRequestParams::new("skill://example/SKILL.md"))
            .await
            .expect_err("skills undeclared"),
        client
            .list_all_skills()
            .await
            .expect_err("skills undeclared"),
    ];
    client.cancel().await.expect("cancel client");
    let requests = raw_peer.await.expect("raw peer task");

    for error in errors {
        assert!(matches!(
            error,
            ServiceError::McpError(error) if error.code == ErrorCode::METHOD_NOT_FOUND
        ));
    }
    assert!(requests.is_empty(), "unexpected requests: {requests:?}");
}

#[tokio::test]
async fn skills_methods_do_not_send_without_peer_info() {
    assert_no_skills_requests(None).await;
}

#[tokio::test]
async fn skills_methods_do_not_send_without_extension_declaration() {
    let capabilities = ServerCapabilities::builder().enable_resources().build();
    assert_no_skills_requests(Some(InitializeResult::new(capabilities).into())).await;
}
