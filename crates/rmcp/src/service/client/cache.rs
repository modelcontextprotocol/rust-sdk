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
}

impl Default for ClientCacheConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            default_ttl: Duration::ZERO,
            max_ttl: MAX_CLIENT_CACHE_TTL,
            private_partition: None,
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
    scope: CacheScope,
}

#[derive(Debug)]
pub(crate) struct PeerResponseCacheState<R: ServiceRole> {
    entries: HashMap<CacheKey, CachedPeerResponse<R::PeerResp>>,
    config: ClientCacheConfig,
}

impl<R: ServiceRole> Default for PeerResponseCacheState<R> {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            config: ClientCacheConfig::default(),
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

    /// Returns a fresh cached response, preferring the current private partition
    /// before the public partition. Expired entries are removed on access.
    pub(crate) async fn cached_response(&self, logical_key: &str) -> Option<R::PeerResp> {
        let now = Instant::now();
        let mut cache = self.response_cache.write().await;
        if !cache.config.enabled {
            return None;
        }

        let private_key = Self::cache_key(
            logical_key,
            CachePartition::Private(Self::private_partition(&cache.config)),
        );
        if let Some(entry) = cache.entries.get(&private_key) {
            if entry.expires_at > now && entry.scope == CacheScope::Private {
                return Some(entry.value.clone());
            }
        }
        cache.entries.remove(&private_key);

        let public_key = Self::cache_key(logical_key, CachePartition::Public);
        if let Some(entry) = cache.entries.get(&public_key) {
            if entry.expires_at > now && entry.scope == CacheScope::Public {
                return Some(entry.value.clone());
            }
        }
        cache.entries.remove(&public_key);
        None
    }

    /// Stores a response when the configured effective TTL is positive.
    ///
    /// Missing `cacheScope` is treated as private. This is deliberately more
    /// conservative than the model's backwards-compatible wire default and
    /// prevents an older or malformed server response from becoming shareable.
    pub(crate) async fn cache_response(
        &self,
        logical_key: String,
        value: R::PeerResp,
        ttl_ms: Option<u64>,
        cache_scope: Option<CacheScope>,
    ) {
        let now = Instant::now();
        let mut cache = self.response_cache.write().await;
        if !cache.config.enabled {
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

        cache.entries.retain(|_, entry| entry.expires_at > now);
        cache.entries.remove(&opposite_key);
        cache.entries.insert(
            target_key,
            CachedPeerResponse {
                value,
                expires_at,
                scope,
            },
        );
    }

    pub(crate) async fn invalidate_cached_responses(&self, prefix: &str) {
        self.response_cache
            .write()
            .await
            .entries
            .retain(|key, _| !key.logical_key.starts_with(prefix));
    }

    pub(crate) async fn invalidate_cached_response(&self, logical_key: &str) {
        self.response_cache
            .write()
            .await
            .entries
            .retain(|key, _| key.logical_key != logical_key);
    }
}

impl Peer<RoleClient> {
    /// Replaces the response-cache configuration.
    ///
    /// Changing the private partition invalidates private entries from the old
    /// authorization context. Disabling the cache clears every entry.
    pub async fn set_response_cache_config(&self, config: ClientCacheConfig) {
        let mut cache = self.response_cache.write().await;
        let partition_changed = cache.config.private_partition != config.private_partition;
        cache.config = config;
        if !cache.config.enabled {
            cache.entries.clear();
        } else if partition_changed {
            cache
                .entries
                .retain(|_, entry| entry.scope == CacheScope::Public);
        }
    }

    /// Returns a snapshot of the active response-cache configuration.
    pub async fn response_cache_config(&self) -> ClientCacheConfig {
        self.response_cache.read().await.config.clone()
    }

    /// Clears every cached client response without changing the configuration.
    pub async fn clear_response_cache(&self) {
        self.response_cache.write().await.entries.clear();
    }
}
