use anyhow::Result;
use bytes::Bytes;
use futures_util::future::BoxFuture;
use moka::future::Cache;
use std::any::Any;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, info};

/// Cache entry with TTL information
#[derive(Debug, Clone)]
struct CacheEntry {
    value: Bytes,
    expires_at: Instant,
}

impl CacheEntry {
    fn new(value: Bytes, ttl: Duration) -> Self {
        Self {
            value,
            expires_at: Instant::now() + ttl,
        }
    }

    fn is_expired(&self) -> bool {
        Instant::now() > self.expires_at
    }
}

/// Specialized entry for zero-cost deserialization on L1
#[derive(Clone)]
pub struct TypedCacheEntry {
    pub value: Arc<dyn Any + Send + Sync>,
    pub expires_at: Instant,
}

impl TypedCacheEntry {
    pub fn new(value: Arc<dyn Any + Send + Sync>, ttl: Duration) -> Self {
        Self {
            value,
            expires_at: Instant::now() + ttl,
        }
    }

    pub fn is_expired(&self) -> bool {
        Instant::now() > self.expires_at
    }
}

/// Configuration for `MokaCache`
#[derive(Debug, Clone, Copy)]
pub struct MokaCacheConfig {
    /// Max capacity of the cache
    pub max_capacity: u64,
    /// Time to live for cache entries
    pub time_to_live: Duration,
    /// Time to idle for cache entries
    pub time_to_idle: Duration,
}

impl Default for MokaCacheConfig {
    fn default() -> Self {
        Self {
            max_capacity: 5000,
            time_to_live: Duration::from_secs(3600),
            time_to_idle: Duration::from_secs(120),
        }
    }
}

/// Moka in-memory cache with per-key TTL support
pub struct MokaCache {
    /// Moka cache instance for raw bytes
    cache: Cache<String, CacheEntry>,
    /// Moka cache instance for typed objects (Zero-cost optimization)
    typed_cache: Cache<String, TypedCacheEntry>,
    /// Hit counter
    hits: Arc<AtomicU64>,
    /// Miss counter
    misses: Arc<AtomicU64>,
    /// Set counter
    sets: Arc<AtomicU64>,
    /// Coalesced requests counter
    #[allow(dead_code)]
    coalesced_requests: Arc<AtomicU64>,
}

impl MokaCache {
    /// Create new Moka cache
    pub fn new(config: MokaCacheConfig) -> Result<Self> {
        info!("Initializing Moka Cache");

        let cache = Cache::builder()
            .max_capacity(config.max_capacity)
            .time_to_live(config.time_to_live)
            .time_to_idle(config.time_to_idle)
            .build();

        let typed_cache = Cache::builder()
            .max_capacity(config.max_capacity)
            .time_to_live(config.time_to_live)
            .time_to_idle(config.time_to_idle)
            .build();

        info!(
            capacity = config.max_capacity,
            "Moka Cache initialized with Byte and Typed storage"
        );

        Ok(Self {
            cache,
            typed_cache,
            hits: Arc::new(AtomicU64::new(0)),
            misses: Arc::new(AtomicU64::new(0)),
            sets: Arc::new(AtomicU64::new(0)),
            coalesced_requests: Arc::new(AtomicU64::new(0)),
        })
    }

    /// Set a typed value in the L1 cache (zero-cost optimization)
    pub async fn set_typed(
        &self,
        key: &str,
        value: Arc<dyn Any + Send + Sync>,
        ttl: Duration,
    ) -> Result<()> {
        let entry = TypedCacheEntry::new(value, ttl);
        self.typed_cache.insert(key.to_string(), entry).await;
        self.sets.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Get a typed value from the L1 cache
    pub async fn get_typed(&self, key: &str) -> Option<Arc<dyn Any + Send + Sync>> {
        if let Some(entry) = self.typed_cache.get(key).await {
            if entry.is_expired() {
                let _ = self.typed_cache.remove(key).await;
                self.misses.fetch_add(1, Ordering::Relaxed);
                None
            } else {
                self.hits.fetch_add(1, Ordering::Relaxed);
                Some(entry.value)
            }
        } else {
            None
        }
    }
}

// ===== Trait Implementations =====

use crate::traits::{CacheBackend, L2CacheBackend};

/// Implement `CacheBackend` trait for `MokaCache`
impl CacheBackend for MokaCache {
    fn get<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Option<Bytes>> {
        Box::pin(async move {
            if let Some(entry) = self.cache.get(key).await {
                if entry.is_expired() {
                    let _ = self.cache.remove(key).await;
                    self.misses.fetch_add(1, Ordering::Relaxed);
                    None
                } else {
                    self.hits.fetch_add(1, Ordering::Relaxed);
                    Some(entry.value)
                }
            } else {
                self.misses.fetch_add(1, Ordering::Relaxed);
                None
            }
        })
    }

    fn set_with_ttl<'a>(
        &'a self,
        key: &'a str,
        value: Bytes,
        ttl: Duration,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let entry = CacheEntry::new(value, ttl);
            self.cache.insert(key.to_string(), entry).await;
            self.sets.fetch_add(1, Ordering::Relaxed);
            debug!(key = %key, ttl_secs = %ttl.as_secs(), "[Moka] Cached key bytes with TTL");
            Ok(())
        })
    }

    fn remove<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            self.cache.invalidate(key).await;
            self.typed_cache.invalidate(key).await;
            Ok(())
        })
    }

    fn health_check(&self) -> BoxFuture<'_, bool> {
        Box::pin(async move {
            let test_key = "health_check_moka";
            let test_value = Bytes::from("health_check");

            match self
                .set_with_ttl(test_key, test_value.clone(), Duration::from_secs(60))
                .await
            {
                Ok(()) => match self.get(test_key).await {
                    Some(retrieved) => {
                        let _ = self.remove(test_key).await;
                        retrieved == test_value
                    }
                    None => false,
                },
                Err(_) => false,
            }
        })
    }

    fn remove_pattern<'a>(&'a self, pattern: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            // For Moka (L1), we'll do a full invalidation for pattern requests to ensure consistency.
            // Pattern invalidation is usually relatively rare compared to single-key lookups,
            // so clearing L1 is a safe and robust fallback to ensure no stale data remains.
            self.cache.invalidate_all();
            self.typed_cache.invalidate_all();
            
            // Ensure background invalidation tasks are processed
            self.cache.run_pending_tasks().await;
            self.typed_cache.run_pending_tasks().await;

            debug!(pattern = %pattern, "[Moka] Invalidated all entries due to pattern '{}' request", pattern);
            Ok(())
        })
    }

    fn name(&self) -> &'static str {
        "Moka"
    }
}

impl L2CacheBackend for MokaCache {
    fn get_with_ttl<'a>(
        &'a self,
        key: &'a str,
    ) -> BoxFuture<'a, Option<(Bytes, Option<Duration>)>> {
        Box::pin(async move {
            // Moka doesn't easily expose remaining TTL for an entry
            if let Some(entry) = self.cache.get(key).await {
                if entry.is_expired() {
                    let _ = self.cache.remove(key).await;
                    self.misses.fetch_add(1, Ordering::Relaxed);
                    None
                } else {
                    self.hits.fetch_add(1, Ordering::Relaxed);
                    let now = Instant::now();
                    let remaining = if entry.expires_at > now {
                        Some(entry.expires_at.duration_since(now))
                    } else {
                        None
                    };
                    Some((entry.value, remaining))
                }
            } else {
                self.misses.fetch_add(1, Ordering::Relaxed);
                None
            }
        })
    }
}

/// Cache statistics
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub sets: u64,
    pub coalesced_requests: u64,
    pub size: u64,
}
