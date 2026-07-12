from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def replace_once(path: str, old: str, new: str) -> None:
    file = ROOT / path
    text = file.read_text()
    count = text.count(old)
    if count != 1:
        raise RuntimeError(f"{path}: expected one match, found {count}")
    file.write_text(text.replace(old, new, 1))


# Keep the old direct seeding helper for unit tests while production writes
# require a generation captured before the transport request.
replace_once(
    "crates/rmcp/src/service/client/cache.rs",
    """    pub(crate) async fn cache_response(\n        &self,\n        logical_key: String,\n        value: R::PeerResp,\n        ttl_ms: Option<u64>,\n        cache_scope: Option<CacheScope>,\n        generation: CacheGeneration,\n    ) {\n""",
    """    pub(crate) async fn cache_response_with_generation(\n        &self,\n        logical_key: String,\n        value: R::PeerResp,\n        ttl_ms: Option<u64>,\n        cache_scope: Option<CacheScope>,\n        generation: CacheGeneration,\n    ) {\n""",
)

replace_once(
    "crates/rmcp/src/service/client/cache.rs",
    """    pub(crate) async fn invalidate_cached_responses(&self, prefix: &str) {\n""",
    """    #[cfg(test)]\n    pub(crate) async fn cache_response(\n        &self,\n        logical_key: String,\n        value: R::PeerResp,\n        ttl_ms: Option<u64>,\n        cache_scope: Option<CacheScope>,\n    ) {\n        let generation = self.capture_response_cache_generation().await;\n        self.cache_response_with_generation(\n            logical_key,\n            value,\n            ttl_ms,\n            cache_scope,\n            generation,\n        )\n        .await;\n    }\n\n    pub(crate) async fn invalidate_cached_responses(&self, prefix: &str) {\n""",
)

replace_once(
    "crates/rmcp/src/service/client.rs",
    """pub use cache::{ClientCacheConfig, MAX_CLIENT_CACHE_TTL};\n""",
    """pub use cache::{ClientCacheConfig, MAX_CLIENT_CACHE_TTL};\nuse cache::CacheGeneration;\n""",
)

replace_once(
    "crates/rmcp/src/service/client.rs",
    """    async fn cache_result(\n        &self,\n        cache_key: Option<String>,\n        ttl_ms: Option<u64>,\n        cache_scope: Option<CacheScope>,\n        result: ServerResult,\n    ) {\n        let Some(cache_key) = cache_key else {\n            return;\n        };\n        self.cache_response(cache_key, result, ttl_ms, cache_scope)\n            .await;\n    }\n""",
    """    async fn cache_result(\n        &self,\n        cache_key: Option<String>,\n        ttl_ms: Option<u64>,\n        cache_scope: Option<CacheScope>,\n        generation: CacheGeneration,\n        result: ServerResult,\n    ) {\n        let Some(cache_key) = cache_key else {\n            return;\n        };\n        self.cache_response_with_generation(\n            cache_key,\n            result,\n            ttl_ms,\n            cache_scope,\n            generation,\n        )\n        .await;\n    }\n""",
)

# resources/read: capture after a miss and before crossing the transport.
replace_once(
    "crates/rmcp/src/service/client.rs",
    """        let result = self\n            .send_request(ClientRequest::ReadResourceRequest(ReadResourceRequest {\n""",
    """        let generation = self.capture_response_cache_generation().await;\n        let result = self\n            .send_request(ClientRequest::ReadResourceRequest(ReadResourceRequest {\n""",
)
replace_once(
    "crates/rmcp/src/service/client.rs",
    """                    result.cache_scope,\n                    ServerResult::ReadResourceResult(result.clone()),\n""",
    """                    result.cache_scope,\n                    generation,\n                    ServerResult::ReadResourceResult(result.clone()),\n""",
)

# The four list methods all use the same miss -> capture -> request pattern.
for marker in [
    "let uses_cursor = request_uses_cursor(&params);",
]:
    text = (ROOT / "crates/rmcp/src/service/client.rs").read_text()
    expected = 4
    actual = text.count(marker)
    if actual != expected:
        raise RuntimeError(f"client.rs: expected {expected} cursor markers, found {actual}")
    text = text.replace(
        marker,
        "let generation = self.capture_response_cache_generation().await;\n        " + marker,
    )
    (ROOT / "crates/rmcp/src/service/client.rs").write_text(text)

# Add the generation argument to the four list cache writes.
client = ROOT / "crates/rmcp/src/service/client.rs"
text = client.read_text()
for variant in [
    "ListPromptsResult",
    "ListResourcesResult",
    "ListResourceTemplatesResult",
    "ListToolsResult",
]:
    old = f"""                    result.cache_scope,\n                    ServerResult::{variant}(result.clone()),\n"""
    new = f"""                    result.cache_scope,\n                    generation,\n                    ServerResult::{variant}(result.clone()),\n"""
    count = text.count(old)
    if count != 1:
        raise RuntimeError(f"client.rs: expected one {variant} cache write, found {count}")
    text = text.replace(old, new, 1)
client.write_text(text)

# Add regression tests for stale in-flight writes and the entry bound.
replace_once(
    "crates/rmcp/src/service/client.rs",
    """    #[test]\n    fn mrtr_retry_parameters_are_not_cacheable() {\n""",
    """    #[tokio::test]\n    async fn invalidation_suppresses_an_in_flight_cache_write() {\n        let peer = disconnected_peer();\n        let key = list_response_cache_key(TOOL_LIST_CACHE_PREFIX, &None);\n        let generation = peer.capture_response_cache_generation().await;\n        peer.invalidate_tool_cache().await;\n        peer.cache_response_with_generation(\n            key.clone(),\n            ServerResult::ListToolsResult(tools_result(\n                Some(5_000),\n                Some(CacheScope::Private),\n            )),\n            Some(5_000),\n            Some(CacheScope::Private),\n            generation,\n        )\n        .await;\n\n        assert!(peer.cached_response(&key).await.is_none());\n    }\n\n    #[tokio::test]\n    async fn entry_limit_evicts_the_oldest_response() {\n        let peer = disconnected_peer();\n        peer.set_response_cache_config(\n            ClientCacheConfig::default().with_max_entries(1),\n        )\n        .await;\n        let first = resource_read_cache_key_for_uri(\"file:///first\");\n        let second = resource_read_cache_key_for_uri(\"file:///second\");\n        peer.cache_response(\n            first.clone(),\n            ServerResult::ReadResourceResult(ReadResourceResult::new(Vec::new())),\n            Some(5_000),\n            Some(CacheScope::Private),\n        )\n        .await;\n        tokio::time::sleep(Duration::from_millis(1)).await;\n        peer.cache_response(\n            second.clone(),\n            ServerResult::ReadResourceResult(ReadResourceResult::new(Vec::new())),\n            Some(5_000),\n            Some(CacheScope::Private),\n        )\n        .await;\n\n        assert!(peer.cached_response(&first).await.is_none());\n        assert!(peer.cached_response(&second).await.is_some());\n    }\n\n    #[test]\n    fn mrtr_retry_parameters_are_not_cacheable() {\n""",
)

# Document the final two guarantees.
docs = ROOT / "docs/CLIENT_CACHING.md"
text = docs.read_text()
text = text.replace(
    "Use `ClientCacheConfig::disabled()` to disable cache reads and writes, or\n`clear_response_cache()` to clear held responses without changing the policy.\n",
    "Use `ClientCacheConfig::disabled()` to disable cache reads and writes, or\n`clear_response_cache()` to clear held responses without changing the policy.\n`with_max_entries()` bounds the in-memory store; the default is 512 entries and\na value of zero removes the limit.\n",
)
text += "\nCache invalidation advances an internal generation. Responses from requests that\nwere already in flight before an invalidation or authorization-context change are\nnot written back into the cache.\n"
docs.write_text(text)

print("issue 974 finalization edits applied")
