//! Moka Cache - In-Memory Cache Backend
//!
//! High-performance in-memory cache using Moka for hot data storage.

use anyhow::Result;
use moka::future::Cache;
use serde_json;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, info};

/// Cache entry with TTL information
#[derive(Debug, Clone)]
struct CacheEntry {
    value: serde_json::Value,
    expires_at: Instant,
}

impl CacheEntry {
    fn new(value: serde_json::Value, ttl: Duration) -> Self {
        Self {
            value,
            expires_at: Instant::now() + ttl,
        }
    }

    fn is_expired(&self) -> bool {
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
            max_capacity: 2000,
            time_to_live: Duration::from_secs(3600),
            time_to_idle: Duration::from_secs(120),
        }
    }
}

/// Moka in-memory cache with per-key TTL support
///
/// This is the default L1 (hot tier) cache backend, providing:
/// - Fast in-memory access (< 1ms latency)
/// - Automatic eviction via LRU
/// - Per-key TTL support
/// - Statistics tracking
pub struct MokaCache {
    /// Moka cache instance
    cache: Cache<String, CacheEntry>,
    /// Hit counter
    hits: Arc<AtomicU64>,
    /// Miss counter
    misses: Arc<AtomicU64>,
    /// Set counter
    sets: Arc<AtomicU64>,
    /// Coalesced requests counter (requests that waited for an ongoing computation)
    #[allow(dead_code)]
    coalesced_requests: Arc<AtomicU64>,
}

impl MokaCache {
    /// Create new Moka cache
    /// # Errors
    ///
    /// Returns an error if the cache cannot be initialized.
    pub fn new(config: MokaCacheConfig) -> Result<Self> {
        info!("Initializing Moka Cache");

        let cache = Cache::builder()
            .max_capacity(config.max_capacity)
            .time_to_live(config.time_to_live)
            .time_to_idle(config.time_to_idle)
            .build();

        info!(
            capacity = config.max_capacity,
            "Moka Cache initialized with per-key TTL support"
        );

        Ok(Self {
            cache,
            hits: Arc::new(AtomicU64::new(0)),
            misses: Arc::new(AtomicU64::new(0)),
            sets: Arc::new(AtomicU64::new(0)),
            coalesced_requests: Arc::new(AtomicU64::new(0)),
        })
    }
}

// ===== Trait Implementations =====

use crate::traits::CacheBackend;
use async_trait::async_trait;

/// Implement `CacheBackend` trait for `MokaCache`
///
/// This allows `MokaCache` to be used as a pluggable backend in the multi-tier cache system.
#[async_trait]
impl CacheBackend for MokaCache {
    async fn get(&self, key: &str) -> Option<serde_json::Value> {
        if let Some(entry) = self.cache.get(key).await {
            if entry.is_expired() {
                // Remove expired entry
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
    }

    async fn set_with_ttl(&self, key: &str, value: serde_json::Value, ttl: Duration) -> Result<()> {
        let entry = CacheEntry::new(value, ttl);
        self.cache.insert(key.to_string(), entry).await;
        self.sets.fetch_add(1, Ordering::Relaxed);
        debug!(key = %key, ttl_secs = %ttl.as_secs(), "[Moka] Cached key with TTL");
        Ok(())
    }

    async fn remove(&self, key: &str) -> Result<()> {
        self.cache.remove(key).await;
        Ok(())
    }

    async fn health_check(&self) -> bool {
        // Test basic functionality with custom TTL
        let test_key = "health_check_moka";
        let test_value = serde_json::json!({"test": true});

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
    }

    fn name(&self) -> &'static str {
        "Moka"
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
