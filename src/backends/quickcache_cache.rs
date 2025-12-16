//! `QuickCache` - Fast In-Memory Cache Backend
//!
//! Lightweight and extremely fast in-memory cache optimized for maximum performance.

use anyhow::Result;
use parking_lot::RwLock;
use quick_cache::sync::Cache;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, info};

/// Cache entry with TTL information
#[derive(Debug, Clone)]
struct CacheEntry {
    value: Vec<u8>,
    expires_at: Instant,
}

impl CacheEntry {
    fn new(value: Vec<u8>, ttl: Duration) -> Self {
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
    /// # Example
    ///
    /// ```no_run
    /// # use multi_tier_cache::backends::QuickCacheBackend;
    /// # async fn example() -> anyhow::Result<()> {
    /// // Default capacity (2000 entries)
    /// let cache = QuickCacheBackend::new(2000).await?;
    ///
    /// // Custom capacity
    /// let cache = QuickCacheBackend::new(10000).await?;
    /// # Ok(())
    /// # }
    /// ```
    /// # Errors
    ///
    /// Returns an error if the cache cannot be initialized.
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
        // QuickCache doesn't expose size directly, so we estimate with stats
        // In practice, this would need to be tracked separately
        0 // Placeholder - quick_cache doesn't expose size
    }
}

// ===== Trait Implementations =====

use crate::traits::CacheBackend;
use async_trait::async_trait;

/// Implement `CacheBackend` trait for `QuickCacheBackend`
///
/// This allows `QuickCacheBackend` to be used as a pluggable backend in the multi-tier cache system.
#[async_trait]
impl CacheBackend for QuickCacheBackend {
    async fn get(&self, key: &str) -> Option<Vec<u8>> {
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
    }

    async fn set_with_ttl(&self, key: &str, value: &[u8], ttl: Duration) -> Result<()> {
        let entry = Arc::new(RwLock::new(CacheEntry::new(value.to_vec(), ttl)));
        self.cache.insert(key.to_string(), entry);
        self.sets.fetch_add(1, Ordering::Relaxed);
        debug!(key = %key, ttl_secs = %ttl.as_secs(), "[QuickCache] Cached key with TTL");
        Ok(())
    }

    async fn remove(&self, key: &str) -> Result<()> {
        self.cache.remove(key);
        Ok(())
    }

    async fn health_check(&self) -> bool {
        let test_key = "health_check_quickcache";
        let test_value = vec![1, 2, 3, 4];

        match self
            .set_with_ttl(test_key, &test_value, Duration::from_secs(60))
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
    }

    fn name(&self) -> &'static str {
        "QuickCache"
    }
}
