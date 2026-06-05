use rmcp::model::{CacheScope, ListToolsResult, ReadResourceResult, ResourceContents};
use serde_json::json;

#[test]
fn paginated_results_serialize_cache_hints_in_meta() {
    let result = ListToolsResult::with_all_items(Vec::new())
        .with_ttl_ms(5_000)
        .with_cache_scope(CacheScope::User);

    let actual = serde_json::to_value(result).expect("serialize list tools result");

    assert_eq!(
        actual,
        json!({
            "_meta": {
                "ttlMs": 5000,
                "cacheScope": "user"
            },
            "tools": []
        })
    );
}

#[test]
fn read_resource_results_serialize_cache_hints_in_content_meta() {
    let result =
        ReadResourceResult::new(vec![ResourceContents::text("hello", "file:///example.txt")])
            .with_ttl_ms(10_000)
            .with_cache_scope(CacheScope::Shared);

    let actual = serde_json::to_value(result).expect("serialize read resource result");

    assert_eq!(actual["contents"][0]["_meta"]["ttlMs"], 10000);
    assert_eq!(actual["contents"][0]["_meta"]["cacheScope"], "shared");
}
