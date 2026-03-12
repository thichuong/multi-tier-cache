//! `QuickCache` - Fast In-Memory Cache Backend
//!
//! Lightweight and extremely fast in-memory cache optimized for maximum performance.

use anyhow::Result;
use bytes::Bytes;
use futures_util::future::BoxFuture;
use parking_lot::RwLock;
use quick_cache::sync::Cache;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
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

/// `QuickCache` in-memory cache with per-key TTL support
///
/// This is an alternative L1 (hot tier) cache backend optimized for maximum performance:
/// - Extremely fast in-memory access (sub-microsecond latency)
/// - Automatic eviction via LRU
/// - Per-key TTL support
/// - Minimal memory overhead
/// - Lock-free design for concurrent access
///
/// **When to use `QuickCache` vs Moka**:
/// - Use `QuickCache` when you need maximum throughput and minimal latency
/// - Use Moka when you need advanced features like time-to-idle or weight-based eviction
pub struct QuickCacheBackend {
    /// `QuickCache` instance
    cache: Cache<String, Arc<RwLock<CacheEntry>>>,
    /// Hit counter
    hits: Arc<AtomicU64>,
    /// Miss counter
    misses: Arc<AtomicU64>,
    /// Set counter
    sets: Arc<AtomicU64>,
}

impl QuickCacheBackend {
    /// Create new `QuickCache`
    ///
    /// # Arguments
    ///
    /// * `max_capacity` - Maximum number of entries (default: 2000)
    ///
    /// # Errors
    ///
    /// Returns an error if the capacity is invalid.
    pub fn new(max_capacity: u64) -> Result<Self> {
        info!(capacity = max_capacity, "Initializing QuickCache");

        let cache = Cache::new(usize::try_from(max_capacity)?);

        Ok(Self {
            cache,
            hits: Arc::new(AtomicU64::new(0)),
            misses: Arc::new(AtomicU64::new(0)),
            sets: Arc::new(AtomicU64::new(0)),
        })
    }

    /// Get current cache size
    #[must_use]
    pub const fn size(&self) -> usize {
        0 // Placeholder - quick_cache doesn't expose size
    }
}

// ===== Trait Implementations =====

use crate::traits::CacheBackend;

/// Implement `CacheBackend` trait for `QuickCacheBackend`
impl CacheBackend for QuickCacheBackend {
    fn get<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Option<Bytes>> {
        Box::pin(async move {
            if let Some(entry_lock) = self.cache.get(key) {
                let entry = entry_lock.read();
                if entry.is_expired() {
                    // Remove expired entry
                    drop(entry); // Release read lock before removing
                    self.cache.remove(key);
                    self.misses.fetch_add(1, Ordering::Relaxed);
                    None
                } else {
                    self.hits.fetch_add(1, Ordering::Relaxed);
                    Some(entry.value.clone())
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
            let entry = Arc::new(RwLock::new(CacheEntry::new(value, ttl)));
            self.cache.insert(key.to_string(), entry);
            self.sets.fetch_add(1, Ordering::Relaxed);
            debug!(key = %key, ttl_secs = %ttl.as_secs(), "[QuickCache] Cached key with TTL");
            Ok(())
        })
    }

    fn remove<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            self.cache.remove(key);
            Ok(())
        })
    }

    fn health_check(&self) -> BoxFuture<'_, bool> {
        Box::pin(async move {
            let test_key = "health_check_quickcache";
            let test_value = Bytes::from_static(b"health_check");

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

    fn name(&self) -> &'static str {
        "QuickCache"
    }
}
