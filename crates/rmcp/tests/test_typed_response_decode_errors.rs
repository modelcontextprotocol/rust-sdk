#![cfg(all(feature = "client", feature = "transport-async-rw"))]

use std::{error::Error, time::Duration};

use rmcp::{
    ClientLifecycleMode, ClientServiceExt,
    model::{ClientRequest, CustomRequest, InitializeResult, ProtocolVersion, ServerResult},
    service::{ClientInitializeError, RoleClient, ServiceError, serve_directly},
};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream},
    task::LocalSet,
};

async fn reply(peer: &mut BufReader<DuplexStream>, method: &str, mut response: Value) {
    let mut line = String::new();
    assert_ne!(peer.read_line(&mut line).await.unwrap(), 0);
    let request: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(request["method"], method);
    assert!(!request["id"].is_null(), "expected a request ID");
    response["jsonrpc"] = json!("2.0");
    response["id"] = request["id"].clone();
    peer.get_mut()
        .write_all(format!("{response}\n").as_bytes())
        .await
        .unwrap();
}

fn assert_decode_error(error: &dyn Error, method: &str, path: Option<&str>, field: &str) {
    let message = error.to_string();
    let source = error
        .source()
        .and_then(|source| source.downcast_ref::<serde_json::Error>())
        .unwrap_or_else(|| panic!("expected a Serde error source, got {message}"));
    assert!(source.is_data(), "{source}");
    assert!(
        source
            .to_string()
            .contains(&format!("missing field `{field}`")),
        "{source}",
    );
    assert!(message.contains(method), "{message}");
    if let Some(path) = path {
        assert!(message.contains(path), "{message}");
    }
}

#[tokio::test]
async fn list_tools_keeps_the_decode_error_and_can_read_the_next_reply() {
    LocalSet::new()
        .run_until(async {
            let (transport, peer) = tokio::io::duplex(4096);
            let mut peer = BufReader::new(peer);
            let client = serve_directly::<RoleClient, _, _, _, _>(
                (),
                transport,
                Some(InitializeResult::default().into()),
            );
            let respond = async {
                reply(
                    &mut peer,
                    "tools/list",
                    json!({
                        "result": { "tools": [
                            { "name": "valid", "inputSchema": { "type": "object" } },
                            { "name": "missing-schema" }
                        ] }
                    }),
                )
                .await;
                reply(
                    &mut peer,
                    "tools/list",
                    json!({
                        "result": { "tools": [
                            { "name": "valid", "inputSchema": { "type": "object" } }
                        ] }
                    }),
                )
                .await;
            };
            let list = async { (client.list_tools(None).await, client.list_tools(None).await) };
            let ((), (malformed, valid)) = tokio::join!(respond, list);
            let valid = valid.expect("the next well-formed reply should succeed");
            assert_eq!(valid.tools.len(), 1);
            assert_eq!(valid.tools[0].name, "valid");
            assert_decode_error(
                &malformed.unwrap_err(),
                "tools/list",
                Some("tools[1]"),
                "inputSchema",
            );
        })
        .await;
}

#[rstest::rstest]
#[case("prompts/list", json!({ "prompts": [{ "description": "missing name" }] }), "prompts[0]", "name")]
#[case("resources/list", json!({ "resources": [{ "name": "missing URI" }] }), "resources[0]", "uri")]
#[case("resources/templates/list", json!({ "resourceTemplates": [{ "name": "missing URI template" }] }), "resourceTemplates[0]", "uriTemplate")]
#[tokio::test]
async fn other_typed_lists_keep_the_decode_error(
    #[case] method: &str,
    #[case] malformed: Value,
    #[case] path: &str,
    #[case] field: &str,
) {
    LocalSet::new()
        .run_until(async {
            let (transport, peer) = tokio::io::duplex(4096);
            let mut peer = BufReader::new(peer);
            let client = serve_directly::<RoleClient, _, _, _, _>(
                (),
                transport,
                Some(InitializeResult::default().into()),
            );
            let list = async {
                match method {
                    "prompts/list" => client.list_prompts(None).await.map(drop),
                    "resources/list" => client.list_resources(None).await.map(drop),
                    "resources/templates/list" => {
                        client.list_resource_templates(None).await.map(drop)
                    }
                    _ => unreachable!(),
                }
            };
            let ((), result) = tokio::join!(
                reply(&mut peer, method, json!({ "result": malformed })),
                list
            );
            assert_decode_error(&result.unwrap_err(), method, Some(path), field);
        })
        .await;
}

async fn startup_error(
    lifecycle: ClientLifecycleMode,
    replies: impl IntoIterator<Item = (&'static str, Value)>,
) -> ClientInitializeError {
    LocalSet::new()
        .run_until(async {
            let (transport, peer) = tokio::io::duplex(4096);
            let mut peer = BufReader::new(peer);
            let respond = async {
                for (method, response) in replies {
                    reply(&mut peer, method, response).await;
                }
            };
            let ((), startup) = tokio::time::timeout(Duration::from_secs(10), async {
                tokio::join!(respond, ().serve_with_lifecycle(transport, lifecycle))
            })
            .await
            .expect("startup should complete after the scripted replies");
            startup.expect_err("malformed startup reply should fail")
        })
        .await
}

fn malformed_initialize_reply() -> Value {
    json!({ "result": { "protocolVersion": "2025-11-25", "capabilities": {} } })
}

fn auto_lifecycle() -> ClientLifecycleMode {
    ClientLifecycleMode::Auto {
        preferred_versions: vec![ProtocolVersion::V_2026_07_28],
        legacy_version: Some(ProtocolVersion::V_2025_11_25),
    }
}

#[tokio::test]
async fn initialize_keeps_the_decode_error() {
    let error = startup_error(
        ClientLifecycleMode::Initialize,
        [("initialize", malformed_initialize_reply())],
    )
    .await;
    assert_decode_error(&error, "initialize", None, "serverInfo");
}

#[rstest::rstest]
#[case::discover(ClientLifecycleMode::Discover {
    preferred_versions: vec![ProtocolVersion::V_2026_07_28],
})]
#[case::auto(auto_lifecycle())]
#[tokio::test]
async fn malformed_discovery_keeps_the_decode_error(#[case] lifecycle: ClientLifecycleMode) {
    let error = startup_error(
        lifecycle,
        [(
            "server/discover",
            json!({ "result": {
                "resultType": "complete",
                "capabilities": {},
                "ttlMs": 0,
                "cacheScope": "private"
            } }),
        )],
    )
    .await;
    assert_decode_error(&error, "server/discover", None, "supportedVersions");
}

#[tokio::test]
async fn auto_keeps_the_decode_error_when_legacy_initialize_fails() {
    let error = startup_error(
        auto_lifecycle(),
        [
            (
                "server/discover",
                json!({ "error": { "code": -32601, "message": "Method not found" } }),
            ),
            ("initialize", malformed_initialize_reply()),
        ],
    )
    .await;
    let ClientInitializeError::LegacyFallbackFailed {
        discover, fallback, ..
    } = error
    else {
        panic!("expected discovery and legacy errors, got {error}");
    };
    assert!(matches!(*discover, ClientInitializeError::JsonRpcError(_)));
    assert_decode_error(fallback.as_ref(), "initialize", None, "serverInfo");
}

#[tokio::test]
async fn known_wrong_result_and_raw_custom_result_keep_their_existing_behavior() {
    LocalSet::new()
        .run_until(async {
            let (transport, peer) = tokio::io::duplex(4096);
            let mut peer = BufReader::new(peer);
            let client = serve_directly::<RoleClient, _, _, _, _>(
                (),
                transport,
                Some(InitializeResult::default().into()),
            );
            let custom = json!({ "status": "ok", "data": [42] });
            let respond = async {
                reply(
                    &mut peer,
                    "tools/list",
                    json!({ "result": { "prompts": [] } }),
                )
                .await;
                reply(&mut peer, "example/ping", json!({ "result": custom })).await;
            };
            let request = async {
                let wrong = client.list_tools(None).await;
                let raw = client
                    .send_request(ClientRequest::CustomRequest(CustomRequest::new(
                        "example/ping",
                        None,
                    )))
                    .await;
                (wrong, raw)
            };
            let ((), (wrong, raw)) = tokio::join!(respond, request);
            assert!(matches!(wrong, Err(ServiceError::UnexpectedResponse)));
            let ServerResult::CustomResult(raw) = raw.unwrap() else {
                panic!("expected an opaque custom result");
            };
            assert_eq!(raw.0, custom);
        })
        .await;
}
