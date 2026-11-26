use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use super::RoleClient;
use crate::{
    model::{CacheScope, ServerResult},
    service::{Peer, ServiceRole},
};

/// Maximum server-provided cache TTL honoured by the client response cache.
pub const MAX_CLIENT_CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// Configuration for the built-in MCP client response cache.
///
/// A cache is allocated per client [`Peer`]. Public responses may be reused
/// throughout that client connection. Private responses are additionally
/// partitioned by `private_partition`; changing the partition drops every
/// private entry while preserving public entries.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ClientCacheConfig {
    /// Enables cache reads and writes.
    pub enabled: bool,
    /// TTL used when a backwards-compatible server omits `ttlMs`.
    ///
    /// The default is zero, which leaves such responses immediately stale.
    pub default_ttl: Duration,
    /// Upper bound applied to both server-provided and default TTLs.
    ///
    /// A value of zero disables caching; any successful refresh with an effective
    /// TTL of zero also evicts any stale entry retained for that key.
    pub max_ttl: Duration,
    /// Stable opaque identity for the current authorization context.
    ///
    /// A single-principal client may leave this unset because each client owns
    /// its own in-memory store. Gateways or clients that change principals on an
    /// existing connection should set this value and update it whenever the
    /// authorization context changes.
    pub private_partition: Option<String>,
    /// Maximum number of responses retained by the in-memory cache.
    ///
    /// A value of zero disables the size limit.
    pub max_entries: usize,
    /// Serves an expired cached response when a re-fetch fails.
    ///
    /// SEP-2549 permits clients to serve stale responses if errors occur while
    /// re-fetching (for example, network issues or server downtime). When this
    /// is enabled the client retains expired entries so it can fall back to the
    /// last known response instead of surfacing the transport or server error.
    /// A successful re-fetch always overwrites the stale entry.
    pub serve_stale_on_error: bool,
}

impl Default for ClientCacheConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            default_ttl: Duration::ZERO,
            max_ttl: MAX_CLIENT_CACHE_TTL,
            private_partition: None,
            max_entries: 512,
            serve_stale_on_error: true,
        }
    }
}

impl ClientCacheConfig {
    /// Returns a configuration that disables all cache reads and writes.
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            ..Self::default()
        }
    }

    /// Sets the TTL used when a response omits `ttlMs`.
    pub fn with_default_ttl(mut self, default_ttl: Duration) -> Self {
        self.default_ttl = default_ttl;
        self
    }

    /// Sets the maximum TTL the client will honour.
    ///
    /// A zero duration disables caching and causes successful refreshes to evict
    /// any stale cached entry for that key.
    pub fn with_max_ttl(mut self, max_ttl: Duration) -> Self {
        self.max_ttl = max_ttl;
        self
    }

    /// Sets the stable partition for private responses.
    pub fn with_private_partition(mut self, partition: impl Into<String>) -> Self {
        self.private_partition = Some(partition.into());
        self
    }

    /// Sets the maximum number of retained responses.
    pub fn with_max_entries(mut self, max_entries: usize) -> Self {
        self.max_entries = max_entries;
        self
    }

    /// Controls whether an expired response may be served when a re-fetch fails.
    pub fn with_serve_stale_on_error(mut self, serve_stale_on_error: bool) -> Self {
        self.serve_stale_on_error = serve_stale_on_error;
        self
    }
}

/// A cached response whose `ttlMs` can be rewritten when it is served.
pub(crate) trait RemainingTtl {
    /// Replaces a server-provided `ttlMs` with `remaining`; leaves an absent one absent.
    fn set_remaining_ttl(&mut self, remaining: Duration);
}

impl RemainingTtl for ServerResult {
    fn set_remaining_ttl(&mut self, remaining: Duration) {
        let remaining_ms = u64::try_from(remaining.as_millis()).unwrap_or(u64::MAX);
        match self {
            ServerResult::DiscoverResult(crate::model::DiscoverResult { ttl_ms, .. })
            | ServerResult::ListToolsResult(crate::model::ListToolsResult {
                ttl_ms: Some(ttl_ms),
                ..
            })
            | ServerResult::ListPromptsResult(crate::model::ListPromptsResult {
                ttl_ms: Some(ttl_ms),
                ..
            })
            | ServerResult::ListResourcesResult(crate::model::ListResourcesResult {
                ttl_ms: Some(ttl_ms),
                ..
            })
            | ServerResult::ListResourceTemplatesResult(
                crate::model::ListResourceTemplatesResult {
                    ttl_ms: Some(ttl_ms),
                    ..
                },
            )
            | ServerResult::ListSkillsResult(crate::model::ListSkillsResult {
                ttl_ms: Some(ttl_ms),
                ..
            })
            | ServerResult::ReadResourceResult(crate::model::ReadResourceResult {
                ttl_ms: Some(ttl_ms),
                ..
            }) => *ttl_ms = remaining_ms,
            _ => {}
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum CachePartition {
    Public,
    Private(Arc<str>),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CacheKey {
    logical_key: String,
    partition: CachePartition,
}

#[derive(Debug, Clone)]
struct CachedPeerResponse<T> {
    value: T,
    expires_at: Instant,
    inserted_at: Instant,
    scope: CacheScope,
}

impl<T: Clone + RemainingTtl> CachedPeerResponse<T> {
    fn fresh_value(&self, now: Instant) -> T {
        let mut value = self.value.clone();
        value.set_remaining_ttl(self.expires_at.saturating_duration_since(now));
        value
    }

    fn stale_value(&self) -> T {
        let mut value = self.value.clone();
        value.set_remaining_ttl(Duration::ZERO);
        value
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CacheGeneration(u64);

#[derive(Debug)]
pub(crate) struct PeerResponseCacheState<R: ServiceRole> {
    entries: HashMap<CacheKey, CachedPeerResponse<R::PeerResp>>,
    config: ClientCacheConfig,
    generation: u64,
}

impl<R: ServiceRole> Default for PeerResponseCacheState<R> {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            config: ClientCacheConfig::default(),
            generation: 0,
        }
    }
}

impl<R: ServiceRole> PeerResponseCacheState<R> {
    fn trim_to_limit(&mut self) {
        while self.config.max_entries > 0 && self.entries.len() > self.config.max_entries {
            let Some(oldest_key) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.inserted_at)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            self.entries.remove(&oldest_key);
        }
    }
}

pub(crate) type PeerResponseCache<R> = Arc<tokio::sync::RwLock<PeerResponseCacheState<R>>>;

impl<R: ServiceRole> Peer<R> {
    fn private_partition(config: &ClientCacheConfig) -> Arc<str> {
        Arc::from(config.private_partition.as_deref().unwrap_or("connection"))
    }

    fn cache_key(logical_key: &str, partition: CachePartition) -> CacheKey {
        CacheKey {
            logical_key: logical_key.to_owned(),
            partition,
        }
    }

    /// Captures the cache generation before a request crosses the transport.
    ///
    /// Any configuration change, explicit clear, or notification invalidation
    /// advances the generation. A response from an older generation is not
    /// written back, preventing an in-flight stale response from undoing an
    /// invalidation.
    pub(crate) async fn capture_response_cache_generation(&self) -> CacheGeneration {
        CacheGeneration(self.response_cache.read().await.generation)
    }

    /// Returns a fresh cached response, preferring the current private partition
    /// before the public partition. Its `ttlMs` is the time left until expiry.
    ///
    /// Expired entries are removed on access unless `serve_stale_on_error` is
    /// enabled, in which case they are retained so a later re-fetch failure can
    /// fall back to them via [`Peer::stale_cached_response`].
    pub(crate) async fn cached_response(&self, logical_key: &str) -> Option<R::PeerResp>
    where
        R::PeerResp: RemainingTtl,
    {
        let now = Instant::now();
        let mut cache = self.response_cache.write().await;
        if !cache.config.enabled {
            return None;
        }
        let keep_stale = cache.config.serve_stale_on_error;

        let private_key = Self::cache_key(
            logical_key,
            CachePartition::Private(Self::private_partition(&cache.config)),
        );
        let private_fresh = cache.entries.get(&private_key).and_then(|entry| {
            (entry.expires_at > now && entry.scope == CacheScope::Private)
                .then(|| entry.fresh_value(now))
        });
        if let Some(value) = private_fresh {
            return Some(value);
        }
        if !keep_stale {
            cache.entries.remove(&private_key);
        }

        let public_key = Self::cache_key(logical_key, CachePartition::Public);
        let public_fresh = cache.entries.get(&public_key).and_then(|entry| {
            (entry.expires_at > now && entry.scope == CacheScope::Public)
                .then(|| entry.fresh_value(now))
        });
        if let Some(value) = public_fresh {
            return Some(value);
        }
        if !keep_stale {
            cache.entries.remove(&public_key);
        }
        None
    }

    /// Returns a cached response ignoring its TTL, for use as a fallback when a
    /// re-fetch fails (SEP-2549 permits serving stale responses on error). Its
    /// `ttlMs` is zero.
    ///
    /// Returns `None` when the cache is disabled or `serve_stale_on_error` is
    /// turned off. The private partition is preferred over the public one. The
    /// entry is left in place so repeated failures keep serving it until a
    /// successful re-fetch overwrites it or a notification invalidates it.
    pub(crate) async fn stale_cached_response(&self, logical_key: &str) -> Option<R::PeerResp>
    where
        R::PeerResp: RemainingTtl,
    {
        let cache = self.response_cache.read().await;
        if !cache.config.enabled || !cache.config.serve_stale_on_error {
            return None;
        }

        let private_key = Self::cache_key(
            logical_key,
            CachePartition::Private(Self::private_partition(&cache.config)),
        );
        if let Some(entry) = cache.entries.get(&private_key)
            && entry.scope == CacheScope::Private
        {
            return Some(entry.stale_value());
        }

        let public_key = Self::cache_key(logical_key, CachePartition::Public);
        if let Some(entry) = cache.entries.get(&public_key)
            && entry.scope == CacheScope::Public
        {
            return Some(entry.stale_value());
        }
        None
    }

    /// Stores a response when the configured effective TTL is positive, or evicts
    /// any cached entry for the key when the effective TTL is zero.
    ///
    /// Missing `cacheScope` is treated as private. This is deliberately more
    /// conservative than the model's backwards-compatible wire default and
    /// prevents an older or malformed server response from becoming shareable.
    pub(crate) async fn cache_response_with_generation(
        &self,
        logical_key: String,
        value: R::PeerResp,
        ttl_ms: Option<u64>,
        cache_scope: Option<CacheScope>,
        generation: CacheGeneration,
    ) {
        let now = Instant::now();
        let mut cache = self.response_cache.write().await;
        if !cache.config.enabled || generation.0 != cache.generation {
            return;
        }

        let private_key = Self::cache_key(
            &logical_key,
            CachePartition::Private(Self::private_partition(&cache.config)),
        );
        let public_key = Self::cache_key(&logical_key, CachePartition::Public);

        let requested_ttl = ttl_ms
            .map(Duration::from_millis)
            .unwrap_or(cache.config.default_ttl);
        let ttl = requested_ttl.min(cache.config.max_ttl);
        if ttl.is_zero() {
            cache.entries.remove(&private_key);
            cache.entries.remove(&public_key);
            return;
        }
        let Some(expires_at) = now.checked_add(ttl) else {
            return;
        };
        let scope = cache_scope.unwrap_or(CacheScope::Private);
        let (target_key, opposite_key) = match scope {
            CacheScope::Public => (public_key, private_key),
            CacheScope::Private => (private_key, public_key),
        };

        if !cache.config.serve_stale_on_error {
            cache.entries.retain(|_, entry| entry.expires_at > now);
        }
        cache.entries.remove(&opposite_key);

        if cache.config.max_entries > 0
            && !cache.entries.contains_key(&target_key)
            && cache.entries.len() >= cache.config.max_entries
            && let Some(oldest_key) = cache
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.inserted_at)
                .map(|(key, _)| key.clone())
        {
            cache.entries.remove(&oldest_key);
        }

        cache.entries.insert(
            target_key,
            CachedPeerResponse {
                value,
                expires_at,
                inserted_at: now,
                scope,
            },
        );
    }

    #[cfg(test)]
    pub(crate) async fn cache_response(
        &self,
        logical_key: String,
        value: R::PeerResp,
        ttl_ms: Option<u64>,
        cache_scope: Option<CacheScope>,
    ) {
        let generation = self.capture_response_cache_generation().await;
        self.cache_response_with_generation(logical_key, value, ttl_ms, cache_scope, generation)
            .await;
    }

    pub(crate) async fn invalidate_cached_responses(&self, prefix: &str) {
        let mut cache = self.response_cache.write().await;
        cache.generation = cache.generation.wrapping_add(1);
        cache
            .entries
            .retain(|key, _| !key.logical_key.starts_with(prefix));
    }
}

impl Peer<RoleClient> {
    /// Replaces the response-cache configuration.
    ///
    /// Changing the private partition invalidates private entries from the old
    /// authorization context. Disabling the cache clears every entry. Any
    /// configuration change also suppresses writes from requests that were
    /// already in flight under the previous configuration.
    pub async fn set_response_cache_config(&self, config: ClientCacheConfig) {
        let mut cache = self.response_cache.write().await;
        let config_changed = cache.config != config;
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
        cache.trim_to_limit();
    }

    /// Returns a snapshot of the active response-cache configuration.
    pub async fn response_cache_config(&self) -> ClientCacheConfig {
        self.response_cache.read().await.config.clone()
    }

    /// Clears every cached client response without changing the configuration.
    pub async fn clear_response_cache(&self) {
        let mut cache = self.response_cache.write().await;
        cache.generation = cache.generation.wrapping_add(1);
        cache.entries.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ServiceError,
        model::{ListToolsResult, PaginatedRequestParams, Tool},
        service::{
            AtomicU32RequestIdProvider,
            client::{TOOL_LIST_CACHE_PREFIX, list_response_cache_key},
        },
    };

    fn disconnected_peer() -> Peer<RoleClient> {
        let (peer, receiver) =
            Peer::<RoleClient>::new(Arc::new(AtomicU32RequestIdProvider::default()), None);
        drop(receiver);
        peer
    }

    async fn expire_cached_entries(peer: &Peer<RoleClient>) {
        let mut cache = peer.response_cache.write().await;
        for entry in cache.entries.values_mut() {
            entry.expires_at = entry
                .inserted_at
                .checked_sub(Duration::from_secs(1))
                .unwrap_or(entry.inserted_at);
        }
    }

    fn tools_result(
        name: &str,
        ttl_ms: Option<u64>,
        cache_scope: Option<CacheScope>,
    ) -> ListToolsResult {
        let mut result = ListToolsResult::with_all_items(vec![Tool::new(
            name.to_owned(),
            "",
            Arc::new(Default::default()),
        )]);
        result.ttl_ms = ttl_ms;
        result.cache_scope = cache_scope;
        result
    }

    #[tokio::test]
    async fn zero_ttl_refresh_evicts_stale_entry_and_blocks_stale_fallback() {
        let peer = disconnected_peer();
        let params = None::<PaginatedRequestParams>;
        let key = list_response_cache_key(TOOL_LIST_CACHE_PREFIX, &params);

        // Cache an initial response and expire it while retaining it for stale fallback.
        peer.cache_response(
            key.clone(),
            ServerResult::ListToolsResult(tools_result(
                "v1",
                Some(5_000),
                Some(CacheScope::Public),
            )),
            Some(5_000),
            Some(CacheScope::Public),
        )
        .await;
        expire_cached_entries(&peer).await;
        assert!(peer.cached_response(&key).await.is_none());
        assert!(peer.stale_cached_response(&key).await.is_some());

        // A subsequent successful re-fetch returns ttlMs = 0 ("do not cache").
        peer.cache_response(
            key.clone(),
            ServerResult::ListToolsResult(tools_result("v2", Some(0), Some(CacheScope::Public))),
            Some(0),
            Some(CacheScope::Public),
        )
        .await;

        assert!(
            peer.stale_cached_response(&key).await.is_none(),
            "stale v1 entry must be evicted when a refresh returns ttlMs = 0"
        );
        assert!(matches!(
            peer.list_tools(params).await,
            Err(ServiceError::TransportClosed)
        ));
    }

    #[tokio::test]
    async fn zero_ttl_refresh_evicts_stale_entry_across_scope_changes() {
        let peer = disconnected_peer();
        let key = "tools/list:scope-switch".to_owned();

        // Public -> Private/None with ttlMs = 0
        peer.cache_response(
            key.clone(),
            ServerResult::ListToolsResult(tools_result(
                "public-v1",
                Some(5_000),
                Some(CacheScope::Public),
            )),
            Some(5_000),
            Some(CacheScope::Public),
        )
        .await;
        expire_cached_entries(&peer).await;
        assert!(peer.cached_response(&key).await.is_none());
        assert!(peer.stale_cached_response(&key).await.is_some());

        peer.cache_response(
            key.clone(),
            ServerResult::ListToolsResult(tools_result("v2", Some(0), None)),
            Some(0),
            None,
        )
        .await;
        assert!(peer.stale_cached_response(&key).await.is_none());

        // Private -> Public with ttlMs = 0
        peer.cache_response(
            key.clone(),
            ServerResult::ListToolsResult(tools_result(
                "private-v1",
                Some(5_000),
                Some(CacheScope::Private),
            )),
            Some(5_000),
            Some(CacheScope::Private),
        )
        .await;
        expire_cached_entries(&peer).await;
        assert!(peer.cached_response(&key).await.is_none());
        assert!(peer.stale_cached_response(&key).await.is_some());

        peer.cache_response(
            key.clone(),
            ServerResult::ListToolsResult(tools_result("v2", Some(0), Some(CacheScope::Public))),
            Some(0),
            Some(CacheScope::Public),
        )
        .await;
        assert!(peer.stale_cached_response(&key).await.is_none());
    }

    #[tokio::test]
    async fn omitted_ttl_with_zero_default_ttl_evicts_stale_entry() {
        let peer = disconnected_peer();
        let key = "tools/list:omitted-ttl".to_owned();

        peer.cache_response(
            key.clone(),
            ServerResult::ListToolsResult(tools_result(
                "v1",
                Some(5_000),
                Some(CacheScope::Private),
            )),
            Some(5_000),
            Some(CacheScope::Private),
        )
        .await;
        expire_cached_entries(&peer).await;
        assert!(peer.cached_response(&key).await.is_none());
        assert!(peer.stale_cached_response(&key).await.is_some());

        // Default config has default_ttl = Duration::ZERO, so omitting ttlMs yields effective TTL 0.
        peer.cache_response(
            key.clone(),
            ServerResult::ListToolsResult(tools_result("v2", None, None)),
            None,
            None,
        )
        .await;
        assert!(peer.stale_cached_response(&key).await.is_none());
    }

    #[tokio::test]
    async fn zero_ttl_refresh_preserves_other_keys_and_ignores_stale_generation() {
        let peer = disconnected_peer();
        let key_a = "tools/list:a".to_owned();
        let key_b = "tools/list:b".to_owned();

        let old_generation = peer.capture_response_cache_generation().await;
        peer.clear_response_cache().await;

        peer.cache_response(
            key_a.clone(),
            ServerResult::ListToolsResult(tools_result(
                "fresh-a",
                Some(5_000),
                Some(CacheScope::Public),
            )),
            Some(5_000),
            Some(CacheScope::Public),
        )
        .await;
        peer.cache_response(
            key_b.clone(),
            ServerResult::ListToolsResult(tools_result(
                "fresh-b",
                Some(5_000),
                Some(CacheScope::Public),
            )),
            Some(5_000),
            Some(CacheScope::Public),
        )
        .await;

        // An in-flight zero-TTL response from an older generation must not evict fresh-a.
        peer.cache_response_with_generation(
            key_a.clone(),
            ServerResult::ListToolsResult(tools_result("old-a", Some(0), Some(CacheScope::Public))),
            Some(0),
            Some(CacheScope::Public),
            old_generation,
        )
        .await;
        assert!(peer.cached_response(&key_a).await.is_some());

        // A current-generation zero-TTL response evicts key_a without touching key_b.
        peer.cache_response(
            key_a.clone(),
            ServerResult::ListToolsResult(tools_result("new-a", Some(0), Some(CacheScope::Public))),
            Some(0),
            Some(CacheScope::Public),
        )
        .await;
        assert!(peer.cached_response(&key_a).await.is_none());
        assert!(peer.stale_cached_response(&key_a).await.is_none());
        assert!(peer.cached_response(&key_b).await.is_some());
    }
}
