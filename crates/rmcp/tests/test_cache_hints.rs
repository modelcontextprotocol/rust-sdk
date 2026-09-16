use rmcp::model::{CacheScope, ListToolsResult, ReadResourceResult, ResourceContents, ServerResult};
use serde_json::json;

#[test]
fn repro_empty_cache_scope_drops_every_tool_via_untagged_fallthrough() {
    let payload = json!({
        "tools": [{ "name": "search", "inputSchema": { "type": "object" } }],
        "cacheScope": ""
    });

    let direct = serde_json::from_value::<ListToolsResult>(payload.clone());
    assert!(
        direct.is_ok(),
        "ListToolsResult itself must accept an empty cacheScope, got {direct:?}"
    );
    assert_eq!(direct.unwrap().tools.len(), 1);

    let via_server_result: ServerResult =
        serde_json::from_value(payload).expect("ServerResult must deserialize this payload");
    match via_server_result {
        ServerResult::ListToolsResult(r) => assert_eq!(r.tools.len(), 1),
        other => panic!(
            "expected ListToolsResult, got {other:?} \
             (untagged fallthrough silently reinterpreted a valid tools/list result)"
        ),
    }
}

#[test]
fn paginated_results_serialize_cache_hints_as_top_level_fields() {
    let result = ListToolsResult::with_all_items(Vec::new())
        .with_ttl_ms(5_000)
        .with_cache_scope(CacheScope::Private);

    let actual = serde_json::to_value(result).expect("serialize list tools result");

    assert_eq!(
        actual,
        json!({
            "ttlMs": 5000,
            "cacheScope": "private",
            "tools": [],
            "resultType": "complete"
        })
    );
    assert!(actual.get("_meta").is_none());
}

#[test]
fn read_resource_results_serialize_cache_hints_as_top_level_fields() {
    let result =
        ReadResourceResult::new(vec![ResourceContents::text("hello", "file:///example.txt")])
            .with_ttl_ms(10_000)
            .with_cache_scope(CacheScope::Public);

    let actual = serde_json::to_value(result).expect("serialize read resource result");

    assert_eq!(actual["ttlMs"], 10000);
    assert_eq!(actual["cacheScope"], "public");
    assert!(actual["contents"][0].get("_meta").is_none());
}

#[test]
fn cache_hints_are_omitted_when_absent() {
    let result = ListToolsResult::with_all_items(Vec::new());
    let actual = serde_json::to_value(result).expect("serialize list tools result");

    assert_eq!(actual, json!({ "tools": [], "resultType": "complete" }));
}

#[test]
fn cache_hints_default_to_none_and_negative_ttl_is_normalized_to_zero() {
    let absent: ListToolsResult = serde_json::from_value(json!({
        "tools": []
    }))
    .expect("deserialize result without ttlMs");
    assert_eq!(absent.ttl_ms, None);
    assert_eq!(absent.cache_scope, None);

    let negative: ReadResourceResult = serde_json::from_value(json!({
        "ttlMs": -42,
        "cacheScope": "private",
        "contents": []
    }))
    .expect("deserialize result with negative ttlMs");
    assert_eq!(negative.ttl_ms, Some(0));
    assert_eq!(negative.cache_scope, Some(CacheScope::Private));
}

#[test]
fn cache_scope_round_trips() {
    assert_eq!(
        serde_json::to_value(CacheScope::Public).unwrap(),
        json!("public")
    );
    assert_eq!(
        serde_json::to_value(CacheScope::Private).unwrap(),
        json!("private")
    );
    assert_eq!(
        serde_json::from_value::<CacheScope>(json!("private")).unwrap(),
        CacheScope::Private
    );
}
