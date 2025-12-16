//! `DashMap` Cache - Simple Concurrent `HashMap` Backend
//!
//! A lightweight in-memory cache using `DashMap` for concurrent access.
//! This is a reference implementation showing how to create custom cache backends.

use anyhow::Result;
use dashmap::DashMap;
// use serde_json;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, info};

/// Cache entry with expiration tracking
#[derive(Debug, Clone)]
struct CacheEntry {
    value: Vec<u8>,
    expires_at: Option<Instant>,
}

impl CacheEntry {
    fn new(value: Vec<u8>, ttl: Duration) -> Self {
        Self {
            value,
            expires_at: Some(Instant::now() + ttl),
        }
    }

    fn is_expired(&self) -> bool {
        self.expires_at
            .is_some_and(|expires_at| Instant::now() > expires_at)
    }
}

/// Simple concurrent cache using `DashMap`
///
/// **Use Case**: Educational reference, simple concurrent scenarios
///
/// **Features**:
/// - Lock-free concurrent reads/writes
/// - Manual TTL tracking
/// - No automatic eviction (manual cleanup needed)
/// - Minimal memory overhead
///
/// **Limitations**:
/// - No automatic eviction policy (LRU, LFU, etc.)
/// - No size limits (unbounded growth)
/// - Manual TTL cleanup required
///
/// **When to use**:
/// - Learning how to implement cache backends
/// - Simple use cases with predictable data sizes
/// - When you need full control over eviction logic
///
/// **Example**:
/// ```rust
/// use multi_tier_cache::backends::DashMapCache;
/// use multi_tier_cache::traits::CacheBackend;
/// use std::time::Duration;
///
/// # async fn example() -> anyhow::Result<()> {
/// let cache = DashMapCache::new();
/// let value = serde_json::json!({"user": "alice"});
///
/// cache.set_with_ttl("user:1", value.clone(), Duration::from_secs(60)).await?;
/// let cached = cache.get("user:1").await;
/// assert_eq!(cached, Some(value));
/// # Ok(())
/// # }
/// ```
pub struct DashMapCache {
    /// Concurrent `HashMap`
    map: Arc<DashMap<String, CacheEntry>>,
    /// Hit counter
    hits: Arc<AtomicU64>,
    /// Miss counter
    misses: Arc<AtomicU64>,
    /// Set counter
    sets: Arc<AtomicU64>,
}

impl DashMapCache {
    /// Create new `DashMap` cache
    pub fn new() -> Self {
        info!("Initializing DashMap Cache (concurrent HashMap)");

        Self {
            map: Arc::new(DashMap::new()),
            hits: Arc::new(AtomicU64::new(0)),
            misses: Arc::new(AtomicU64::new(0)),
            sets: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Cleanup expired entries (should be called periodically)
    ///
    /// **Note**: `DashMap` doesn't have automatic eviction, so you need to
    /// call this method periodically to remove expired entries.
    pub fn cleanup_expired(&self) -> usize {
        let mut removed = 0;
        self.map.retain(|_, entry| {
            if entry.is_expired() {
                removed += 1;
                false // Remove
            } else {
                true // Keep
            }
        });
        if removed > 0 {
            debug!(count = removed, "[DashMap] Cleaned up expired entries");
        }
        removed
    }

    /// Get current cache size
    #[must_use]
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Check if cache is empty
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

impl Default for DashMapCache {
    fn default() -> Self {
        Self::new()
    }
}

// ===== Trait Implementations =====

use crate::traits::CacheBackend;
use async_trait::async_trait;

/// Implement `CacheBackend` trait for `DashMapCache`
#[async_trait]
impl CacheBackend for DashMapCache {
    async fn get(&self, key: &str) -> Option<Vec<u8>> {
        if let Some(entry) = self.map.get(key) {
            if entry.is_expired() {
                // Remove expired entry
                drop(entry); // Release read lock
                self.map.remove(key);
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
        let entry = CacheEntry::new(value.to_vec(), ttl);
        self.map.insert(key.to_string(), entry);
        self.sets.fetch_add(1, Ordering::Relaxed);
        debug!(key = %key, ttl_secs = %ttl.as_secs(), "[DashMap] Cached key with TTL");
        Ok(())
    }

    async fn remove(&self, key: &str) -> Result<()> {
        self.map.remove(key);
        Ok(())
    }

    async fn health_check(&self) -> bool {
        let test_key = "health_check_dashmap";
        let test_value = b"health_check_value";

        match self
            .set_with_ttl(test_key, test_value, Duration::from_secs(60))
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
        "DashMap"
    }
}
