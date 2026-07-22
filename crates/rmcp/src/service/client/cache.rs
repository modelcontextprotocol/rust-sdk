use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use super::RoleClient;
use crate::{
    model::CacheScope,
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

    fn scoped_cache_key(
        logical_key: &str,
        scope: CacheScope,
        config: &ClientCacheConfig,
    ) -> CacheKey {
        let partition = match scope {
            CacheScope::Public => CachePartition::Public,
            CacheScope::Private => CachePartition::Private(Self::private_partition(config)),
        };
        Self::cache_key(logical_key, partition)
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
    /// before the public partition.
    ///
    /// Expired entries are removed on access unless `serve_stale_on_error` is
    /// enabled, in which case they are retained so a later re-fetch failure can
    /// fall back to them via [`Peer::stale_cached_response`].
    pub(crate) async fn cached_response(&self, logical_key: &str) -> Option<R::PeerResp> {
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
                .then(|| entry.value.clone())
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
                .then(|| entry.value.clone())
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
    /// re-fetch fails (SEP-2549 permits serving stale responses on error).
    ///
    /// Returns `None` when the cache is disabled or `serve_stale_on_error` is
    /// turned off. The private partition is preferred over the public one. The
    /// entry is left in place so repeated failures keep serving it until a
    /// successful re-fetch overwrites it or a notification invalidates it.
    pub(crate) async fn stale_cached_response(&self, logical_key: &str) -> Option<R::PeerResp> {
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
            return Some(entry.value.clone());
        }

        let public_key = Self::cache_key(logical_key, CachePartition::Public);
        if let Some(entry) = cache.entries.get(&public_key)
            && entry.scope == CacheScope::Public
        {
            return Some(entry.value.clone());
        }
        None
    }

    /// Stores a response when the configured effective TTL is positive.
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

        let requested_ttl = ttl_ms
            .map(Duration::from_millis)
            .unwrap_or(cache.config.default_ttl);
        let ttl = requested_ttl.min(cache.config.max_ttl);
        if ttl.is_zero() {
            return;
        }
        let Some(expires_at) = now.checked_add(ttl) else {
            return;
        };
        let scope = cache_scope.unwrap_or(CacheScope::Private);
        let target_key = Self::scoped_cache_key(&logical_key, scope, &cache.config);
        let opposite_key = match scope {
            CacheScope::Public => Self::cache_key(
                &logical_key,
                CachePartition::Private(Self::private_partition(&cache.config)),
            ),
            CacheScope::Private => Self::cache_key(&logical_key, CachePartition::Public),
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
