use crate::error::CacheResult;
use crate::traits::{CacheBackend, L2CacheBackend};
use bytes::Bytes;
use dashmap::DashMap;
use futures_util::future::BoxFuture;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tracing::{debug, info};

/// Cache entry with expiration tracking
#[derive(Debug, Clone)]
struct CacheEntry {
    value: Bytes,
    expires_at: Option<Instant>,
}

impl CacheEntry {
    fn new(value: Bytes, ttl: Duration) -> Self {
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

pub struct DashMapCache {
    /// Concurrent `HashMap`
    map: Arc<DashMap<String, CacheEntry>>,
    /// Optional maximum capacity limit
    max_capacity: Option<usize>,
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
            max_capacity: None,
            hits: Arc::new(AtomicU64::new(0)),
            misses: Arc::new(AtomicU64::new(0)),
            sets: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Create new `DashMap` cache with capacity limit
    #[must_use]
    pub fn new_with_capacity(capacity: usize) -> Self {
        info!(capacity = capacity, "Initializing DashMap Cache with capacity limit");

        Self {
            map: Arc::new(DashMap::with_capacity(capacity)),
            max_capacity: Some(capacity),
            hits: Arc::new(AtomicU64::new(0)),
            misses: Arc::new(AtomicU64::new(0)),
            sets: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Set maximum capacity limit
    #[must_use]
    pub fn with_max_capacity(mut self, capacity: usize) -> Self {
        self.max_capacity = Some(capacity);
        self
    }

    /// Cleanup expired entries
    pub fn cleanup_expired(&self) -> usize {
        let mut removed = 0;
        self.map.retain(|_, entry| {
            if entry.is_expired() {
                removed += 1;
                false
            } else {
                true
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

/// Implement `CacheBackend` trait for `DashMapCache`
impl CacheBackend for DashMapCache {
    fn get<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Option<Bytes>> {
        Box::pin(async move {
            match self.map.get(key) {
                Some(entry) => {
                    if entry.is_expired() {
                        drop(entry);
                        self.map.remove(key);
                        None
                    } else {
                        Some(entry.value.clone())
                    }
                }
                None => None,
            }
        })
    }

    fn set_with_ttl<'a>(
        &'a self,
        key: &'a str,
        value: Bytes,
        ttl: Duration,
    ) -> BoxFuture<'a, CacheResult<()>> {
        Box::pin(async move {
            // If capacity limit reached and not an existing key, evict expired entries
            if let Some(cap) = self.max_capacity
                && self.map.len() >= cap
                && !self.map.contains_key(key)
            {
                self.cleanup_expired();
            }

            let entry = CacheEntry::new(value, ttl);
            self.map.insert(key.to_string(), entry);
            self.sets.fetch_add(1, Ordering::Relaxed);
            debug!(key = %key, ttl_secs = %ttl.as_secs(), "[DashMap] Cached key bytes with TTL");
            Ok(())
        })
    }

    fn remove<'a>(&'a self, key: &'a str) -> BoxFuture<'a, CacheResult<()>> {
        Box::pin(async move {
            self.map.remove(key);
            Ok(())
        })
    }

    fn remove_pattern<'a>(&'a self, pattern: &'a str) -> BoxFuture<'a, CacheResult<()>> {
        Box::pin(async move {
            self.map
                .retain(|key, _| !crate::backends::matches_pattern(key, pattern));
            Ok(())
        })
    }

    fn health_check(&self) -> BoxFuture<'_, bool> {
        Box::pin(async move { true })
    }

    fn name(&self) -> &'static str {
        "DashMap"
    }
}

impl L2CacheBackend for DashMapCache {
    fn get_with_ttl<'a>(
        &'a self,
        key: &'a str,
    ) -> BoxFuture<'a, Option<(Bytes, Option<Duration>)>> {
        Box::pin(async move {
            if let Some(entry) = self.map.get(key) {
                if entry.is_expired() {
                    drop(entry);
                    self.map.remove(key);
                    self.misses.fetch_add(1, Ordering::Relaxed);
                    None
                } else {
                    let now = Instant::now();
                    if let Some(expires_at) = entry.expires_at {
                        let ttl = expires_at.checked_duration_since(now);
                        if ttl.is_none() {
                            // Expired
                            drop(entry);
                            self.map.remove(key);
                            self.misses.fetch_add(1, Ordering::Relaxed);
                            return None;
                        }
                        self.hits.fetch_add(1, Ordering::Relaxed);
                        Some((entry.value.clone(), ttl))
                    } else {
                        self.hits.fetch_add(1, Ordering::Relaxed);
                        Some((entry.value.clone(), None))
                    }
                }
            } else {
                self.misses.fetch_add(1, Ordering::Relaxed);
                None
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_dashmap_cache_basic_and_capacity() {
        let cache = DashMapCache::new_with_capacity(2);
        assert_eq!(cache.len(), 0);

        cache
            .set_with_ttl("k1", Bytes::from("v1"), Duration::from_millis(50))
            .await
            .unwrap();
        cache
            .set_with_ttl("k2", Bytes::from("v2"), Duration::from_secs(10))
            .await
            .unwrap();
        assert_eq!(cache.len(), 2);

        // Wait for k1 to expire
        tokio::time::sleep(Duration::from_millis(60)).await;

        // Inserting k3 when len >= cap triggers cleanup_expired, evicting expired k1
        cache
            .set_with_ttl("k3", Bytes::from("v3"), Duration::from_secs(10))
            .await
            .unwrap();

        assert_eq!(cache.get("k1").await, None);
        assert_eq!(cache.get("k2").await, Some(Bytes::from("v2")));
        assert_eq!(cache.get("k3").await, Some(Bytes::from("v3")));
    }
}
