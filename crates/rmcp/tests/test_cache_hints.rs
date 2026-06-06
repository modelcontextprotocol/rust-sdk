use rmcp::model::{CacheScope, ListToolsResult, ReadResourceResult, ResourceContents};
use serde_json::json;

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
            "tools": []
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
fn ttl_ms_deserialization_normalizes_absent_and_negative_values_to_zero() {
    let absent: ListToolsResult = serde_json::from_value(json!({
        "tools": []
    }))
    .expect("deserialize result without ttlMs");
    assert_eq!(absent.ttl_ms(), 0);
    assert_eq!(absent.cache_scope(), CacheScope::Public);

    let negative: ReadResourceResult = serde_json::from_value(json!({
        "ttlMs": -42,
        "cacheScope": "private",
        "contents": []
    }))
    .expect("deserialize result with negative ttlMs");
    assert_eq!(negative.ttl_ms, 0);
    assert_eq!(negative.cache_scope, CacheScope::Private);
}
