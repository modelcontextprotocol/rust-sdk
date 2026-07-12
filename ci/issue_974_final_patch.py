from pathlib import Path


def replace_once(path: str, old: str, new: str) -> None:
    file = Path(path)
    text = file.read_text()
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{path}: expected one match, found {count}\n--- old ---\n{old}")
    file.write_text(text.replace(old, new, 1))


replace_once(
    "crates/rmcp/src/service/client.rs",
    '''fn list_response_cache_key(prefix: &str, params: &Option<PaginatedRequestParams>) -> String {
    let cursor = params.as_ref().and_then(|params| params.cursor.as_deref());
    let cursor = serde_json::to_string(&cursor)
        .expect("serializing an optional pagination cursor cannot fail");
    format!("{prefix}{cursor}")
}

fn resource_read_cache_key(params: &ReadResourceRequestParams) -> Option<String> {
    if params.input_responses.is_some() || params.request_state.is_some() {
        return None;
    }
    Some(resource_read_cache_key_for_uri(&params.uri))
}

fn resource_read_cache_key_for_uri(uri: &str) -> String {
    let uri = serde_json::to_string(uri).expect("serializing a resource URI cannot fail");
    format!("{RESOURCE_READ_CACHE_PREFIX}{uri}")
}''',
    '''fn list_response_cache_key(prefix: &str, params: &Option<PaginatedRequestParams>) -> String {
    let params = serde_json::to_string(params)
        .expect("serializing pagination request parameters cannot fail");
    format!("{prefix}{params}")
}

fn resource_read_cache_key(params: &ReadResourceRequestParams) -> Option<String> {
    if params.input_responses.is_some() || params.request_state.is_some() {
        return None;
    }
    let serialized = serde_json::to_string(params)
        .expect("serializing resource request parameters cannot fail");
    Some(format!(
        "{}{serialized}",
        resource_read_cache_prefix_for_uri(&params.uri)
    ))
}

fn resource_read_cache_prefix_for_uri(uri: &str) -> String {
    let uri = serde_json::to_string(uri).expect("serializing a resource URI cannot fail");
    format!("{RESOURCE_READ_CACHE_PREFIX}{uri}:")
}''',
)

replace_once(
    "crates/rmcp/src/service/client.rs",
    '''    pub(crate) async fn invalidate_resource_read_cache(&self, uri: &str) {
        self.invalidate_cached_response(&resource_read_cache_key_for_uri(uri))
            .await;
    }''',
    '''    pub(crate) async fn invalidate_resource_read_cache(&self, uri: &str) {
        self.invalidate_cached_responses(&resource_read_cache_prefix_for_uri(uri))
            .await;
    }''',
)

replace_once(
    "crates/rmcp/src/service/client/cache.rs",
    '''        let config_changed = cache.config != config;
        let partition_changed = cache.config.private_partition != config.private_partition;
        cache.config = config;
        if config_changed {
            cache.generation = cache.generation.wrapping_add(1);
        }
        if !cache.config.enabled {
            cache.entries.clear();
        } else if partition_changed {
            cache
                .entries
                .retain(|_, entry| entry.scope == CacheScope::Public);
        }
        cache.trim_to_limit();''',
    '''        let config_changed = cache.config != config;
        let partition_changed = cache.config.private_partition != config.private_partition;
        let ttl_policy_changed = cache.config.default_ttl != config.default_ttl
            || cache.config.max_ttl != config.max_ttl;
        cache.config = config;
        if config_changed {
            cache.generation = cache.generation.wrapping_add(1);
        }
        if !cache.config.enabled || ttl_policy_changed {
            cache.entries.clear();
        } else if partition_changed {
            cache
                .entries
                .retain(|_, entry| entry.scope == CacheScope::Public);
        }
        cache.trim_to_limit();''',
)

replace_once(
    "crates/rmcp/src/service/client.rs",
    '''    #[test]
    fn paginated_pages_have_independent_cache_keys() {
        let first = Some(PaginatedRequestParams::default().with_cursor(Some("page-a".into())));
        let second = Some(PaginatedRequestParams::default().with_cursor(Some("page-b".into())));

        assert_ne!(
            list_response_cache_key(TOOL_LIST_CACHE_PREFIX, &first),
            list_response_cache_key(TOOL_LIST_CACHE_PREFIX, &second)
        );
    }''',
    '''    #[test]
    fn paginated_pages_have_independent_cache_keys() {
        let first = Some(PaginatedRequestParams::default().with_cursor(Some("page-a".into())));
        let second = Some(PaginatedRequestParams::default().with_cursor(Some("page-b".into())));

        assert_ne!(
            list_response_cache_key(TOOL_LIST_CACHE_PREFIX, &first),
            list_response_cache_key(TOOL_LIST_CACHE_PREFIX, &second)
        );
    }

    #[test]
    fn result_affecting_metadata_is_part_of_cache_keys() {
        let mut first_meta = crate::model::Meta::new();
        first_meta.insert("variant".into(), serde_json::json!("a"));
        let mut second_meta = crate::model::Meta::new();
        second_meta.insert("variant".into(), serde_json::json!("b"));

        let first_page = Some(PaginatedRequestParams {
            meta: Some(first_meta.clone()),
            cursor: Some("page".into()),
        });
        let second_page = Some(PaginatedRequestParams {
            meta: Some(second_meta.clone()),
            cursor: Some("page".into()),
        });
        assert_ne!(
            list_response_cache_key(TOOL_LIST_CACHE_PREFIX, &first_page),
            list_response_cache_key(TOOL_LIST_CACHE_PREFIX, &second_page)
        );

        let first_resource =
            ReadResourceRequestParams::new("file:///example").with_meta(first_meta);
        let second_resource =
            ReadResourceRequestParams::new("file:///example").with_meta(second_meta);
        assert_ne!(
            resource_read_cache_key(&first_resource),
            resource_read_cache_key(&second_resource)
        );
    }''',
)

replace_once(
    "crates/rmcp/src/service/client.rs",
    '''    #[tokio::test]
    async fn resource_update_invalidates_only_the_matching_uri() {
        let peer = disconnected_peer();
        let first_key = resource_read_cache_key_for_uri("file:///first");
        let second_key = resource_read_cache_key_for_uri("file:///second");
        for key in [&first_key, &second_key] {
            peer.cache_response(
                key.clone(),
                ServerResult::ReadResourceResult(
                    ReadResourceResult::new(Vec::new())
                        .with_ttl_ms(5_000)
                        .with_cache_scope(CacheScope::Private),
                ),
                Some(5_000),
                Some(CacheScope::Private),
            )
            .await;
        }

        peer.invalidate_resource_read_cache("file:///first").await;

        assert!(peer.cached_response(&first_key).await.is_none());
        assert!(peer.cached_response(&second_key).await.is_some());
    }''',
    '''    #[tokio::test]
    async fn resource_update_invalidates_every_metadata_variant_for_the_matching_uri() {
        let peer = disconnected_peer();
        let first_plain = ReadResourceRequestParams::new("file:///first");
        let mut meta = crate::model::Meta::new();
        meta.insert("variant".into(), serde_json::json!("a"));
        let first_with_meta = ReadResourceRequestParams::new("file:///first").with_meta(meta);
        let second = ReadResourceRequestParams::new("file:///second");
        let first_plain_key = resource_read_cache_key(&first_plain).unwrap();
        let first_meta_key = resource_read_cache_key(&first_with_meta).unwrap();
        let second_key = resource_read_cache_key(&second).unwrap();
        for key in [&first_plain_key, &first_meta_key, &second_key] {
            peer.cache_response(
                key.clone(),
                ServerResult::ReadResourceResult(
                    ReadResourceResult::new(Vec::new())
                        .with_ttl_ms(5_000)
                        .with_cache_scope(CacheScope::Private),
                ),
                Some(5_000),
                Some(CacheScope::Private),
            )
            .await;
        }

        peer.invalidate_resource_read_cache("file:///first").await;

        assert!(peer.cached_response(&first_plain_key).await.is_none());
        assert!(peer.cached_response(&first_meta_key).await.is_none());
        assert!(peer.cached_response(&second_key).await.is_some());
    }''',
)

replace_once(
    "crates/rmcp/src/service/client.rs",
    '''        let first = resource_read_cache_key_for_uri("file:///first");
        let second = resource_read_cache_key_for_uri("file:///second");''',
    '''        let first = resource_read_cache_key(&ReadResourceRequestParams::new("file:///first"))
            .unwrap();
        let second = resource_read_cache_key(&ReadResourceRequestParams::new("file:///second"))
            .unwrap();''',
)

replace_once(
    "crates/rmcp/src/service/client.rs",
    '''    #[tokio::test]
    async fn private_entries_are_isolated_between_client_peers() {''',
    '''    #[tokio::test]
    async fn changing_ttl_policy_invalidates_existing_entries() {
        let peer = disconnected_peer();
        let key = list_response_cache_key(TOOL_LIST_CACHE_PREFIX, &None);
        peer.cache_response(
            key.clone(),
            ServerResult::ListToolsResult(tools_result(Some(60_000), Some(CacheScope::Public))),
            Some(60_000),
            Some(CacheScope::Public),
        )
        .await;
        assert!(peer.cached_response(&key).await.is_some());

        peer.set_response_cache_config(
            ClientCacheConfig::default().with_max_ttl(Duration::from_millis(1)),
        )
        .await;

        assert!(peer.cached_response(&key).await.is_none());
    }

    #[tokio::test]
    async fn private_entries_are_isolated_between_client_peers() {''',
)

path = "crates/rmcp/tests/test_tool_disable_notification.rs"
replace_once(
    path,
    'model::{CallToolResponse, CallToolResult, ServerCapabilities, ServerInfo, Tool},',
    'model::{CacheScope, CallToolResponse, CallToolResult, ServerCapabilities, ServerInfo, Tool},',
)
replace_once(path, '    trigger_enable: Arc<Notify>,\n}', '    trigger_enable: Arc<Notify>,\n    list_count: Arc<AtomicUsize>,\n}')
replace_once(
    path,
    '            trigger_enable: Arc::new(Notify::new()),\n        }',
    '            trigger_enable: Arc::new(Notify::new()),\n            list_count: Arc::new(AtomicUsize::new(0)),\n        }',
)
replace_once(
    path,
    '''        let router = self.router.read().await;
        Ok(rmcp::model::ListToolsResult {
            tools: router.list_all(),
            ..Default::default()
        })''',
    '''        self.list_count.fetch_add(1, Ordering::SeqCst);
        let router = self.router.read().await;
        Ok(rmcp::model::ListToolsResult {
            tools: router.list_all(),
            ..Default::default()
        }
        .with_ttl_ms(60_000)
        .with_cache_scope(CacheScope::Public))''',
)
replace_once(
    path,
    '    let trigger_enable = server.trigger_enable.clone();\n',
    '    let trigger_enable = server.trigger_enable.clone();\n    let list_count = server.list_count.clone();\n',
)
replace_once(
    path,
    '''    let tools = client_service.peer().list_tools(None).await.unwrap();
    assert_eq!(tools.tools.len(), 2);

    trigger_disable.notify_one();''',
    '''    let tools = client_service.peer().list_tools(None).await.unwrap();
    assert_eq!(tools.tools.len(), 2);
    assert_eq!(list_count.load(Ordering::SeqCst), 1);

    let cached_tools = client_service.peer().list_tools(None).await.unwrap();
    assert_eq!(cached_tools.tools.len(), 2);
    assert_eq!(list_count.load(Ordering::SeqCst), 1);

    trigger_disable.notify_one();''',
)
replace_once(
    path,
    '''    assert_eq!(tools.tools.len(), 1);
    assert_eq!(tools.tools[0].name, "tool_b");''',
    '''    assert_eq!(tools.tools.len(), 1);
    assert_eq!(tools.tools[0].name, "tool_b");
    assert_eq!(list_count.load(Ordering::SeqCst), 2);''',
)
replace_once(
    path,
    '''    let tools = client_service.peer().list_tools(None).await.unwrap();
    assert_eq!(tools.tools.len(), 2);

    client_service.cancel().await.unwrap();''',
    '''    let tools = client_service.peer().list_tools(None).await.unwrap();
    assert_eq!(tools.tools.len(), 2);
    assert_eq!(list_count.load(Ordering::SeqCst), 3);

    client_service.cancel().await.unwrap();''',
)

replace_once(
    "docs/CLIENT_CACHING.md",
    '''Cache keys include the method and result-affecting parameters: the cursor for
paginated list methods and the URI for resource reads. MRTR retries containing
`inputResponses` or `requestState` are never cached.''',
    '''Cache keys include the method and all currently result-affecting parameters: the
cursor and `_meta` for paginated list methods, and the URI plus `_meta` for resource
reads. MRTR retries containing `inputResponses` or `requestState` are never cached.
A response that omits `cacheScope` is treated as private rather than made shareable.''',
)
