# Client response caching

The Rust SDK client honours SEP-2549 caching hints for `tools/list`,
`prompts/list`, `resources/list`, `resources/templates/list`, and
`resources/read`.

Each client connection owns an in-memory cache. A response is served from the
cache only while its effective TTL is fresh. The default TTL for responses that
omit `ttlMs` is zero, and every TTL is capped at 24 hours.

```rust,ignore
use std::time::Duration;
use rmcp::{ClientCacheConfig, ServiceExt};

let client = handler.serve(transport).await?;
client
    .set_response_cache_config(
        ClientCacheConfig::default()
            .with_default_ttl(Duration::from_secs(30))
            .with_private_partition(user_id),
    )
    .await;
```

`private_partition` is an opaque stable identity for the current authorization
context. A normal single-principal client may leave it unset because its cache
is not shared with another client. A gateway, or any client that changes the
principal associated with an existing connection, should set it and update it
when the authorization context changes. Updating the partition discards old
private entries while preserving public entries.

Use `ClientCacheConfig::disabled()` to disable cache reads and writes, or
`clear_response_cache()` to clear held responses without changing the policy.
`with_max_entries()` bounds the in-memory store; the default is 512 entries and
a value of zero removes the limit.

Cache keys include the method and all currently result-affecting parameters: the
cursor and `_meta` for paginated list methods, and the URI plus `_meta` for resource
reads. MRTR retries containing `inputResponses` or `requestState` are never cached.
A response that omits `cacheScope` is treated as private rather than made shareable. List-change notifications
invalidate every cached page for the corresponding method, while resource
update notifications invalidate only the matching URI. When a cursor request
fails, all cached pages for that list method are discarded so the next walk can
restart from the beginning.

Cache invalidation advances an internal generation. Responses from requests that
were already in flight before an invalidation or authorization-context change are
not written back into the cache.
