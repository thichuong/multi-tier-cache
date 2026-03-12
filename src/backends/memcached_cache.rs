//! Memcached Cache - Distributed Cache Backend
//!
//! Memcached-based distributed cache for warm data storage with simple key-value operations.

use anyhow::{Result, anyhow};
use bytes::Bytes;
use futures_util::future::BoxFuture;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tracing::{debug, info};

/// Memcached distributed cache
///
/// This is an alternative L2 (warm tier) cache backend, providing:
/// - Distributed caching across multiple instances
/// - Simple key-value storage (no persistence)
/// - LRU eviction policy
/// - High throughput for read-heavy workloads
///
/// **Note**: Unlike Redis, Memcached does not support:
/// - TTL introspection (cannot get remaining TTL)
/// - Persistence to disk
/// - Advanced data structures
pub struct MemcachedCache {
    /// Memcached client
    client: memcache::Client,
    /// Hit counter
    hits: Arc<AtomicU64>,
    /// Miss counter
    misses: Arc<AtomicU64>,
    /// Set counter
    sets: Arc<AtomicU64>,
}

impl MemcachedCache {
    /// Create new Memcached cache
    ///
    /// # Configuration
    ///
    /// Uses `MEMCACHED_URL` environment variable or defaults to `memcache://127.0.0.1:11211`
    ///
    /// # Errors
    ///
    /// Returns an error if the Memcached client cannot be created.
    pub fn new() -> Result<Self> {
        info!("Initializing Memcached Cache");

        // Get Memcached URL from environment
        let memcached_url = std::env::var("MEMCACHED_URL")
            .unwrap_or_else(|_| "memcache://127.0.0.1:11211".to_string());

        // Create Memcached client
        let client = memcache::connect(memcached_url.as_str())
            .map_err(|e| anyhow!("Failed to connect to Memcached: {e}"))?;

        // Test connection with version command
        match client.version() {
            Ok(versions) => {
                info!(
                    url = %memcached_url,
                    server_count = versions.len(),
                    "Memcached Cache connected successfully"
                );
            }
            Err(e) => {
                return Err(anyhow!("Memcached connection test failed: {e}"));
            }
        }

        Ok(Self {
            client,
            hits: Arc::new(AtomicU64::new(0)),
            misses: Arc::new(AtomicU64::new(0)),
            sets: Arc::new(AtomicU64::new(0)),
        })
    }

    /// Get cache statistics (from Memcached server)
    ///
    /// Returns server statistics like hits, misses, uptime, etc.
    /// Each tuple contains (`server_address`, `stats_map`)
    ///
    /// # Errors
    ///
    /// Returns an error if the stats cannot be retrieved.
    pub fn get_server_stats(
        &self,
    ) -> Result<Vec<(String, std::collections::HashMap<String, String>)>> {
        self.client
            .stats()
            .map_err(|e| anyhow!("Failed to get Memcached stats: {e}"))
    }
}

// ===== Trait Implementations =====

use crate::traits::CacheBackend;

/// Implement `CacheBackend` trait for `MemcachedCache`
impl CacheBackend for MemcachedCache {
    fn get<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Option<Bytes>> {
        Box::pin(async move {
            if let Ok(Some(bytes_vec)) = self.client.get::<Vec<u8>>(key) {
                self.hits.fetch_add(1, Ordering::Relaxed);
                Some(Bytes::from(bytes_vec))
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
            self.client
                .set(
                    key,
                    value.to_vec(),
                    u32::try_from(ttl.as_secs()).unwrap_or(u32::MAX),
                )
                .map_err(|e| anyhow!("Memcached SET failed: {e}"))?;

            self.sets.fetch_add(1, Ordering::Relaxed);
            debug!(key = %key, ttl_secs = %ttl.as_secs(), "[Memcached] Cached key with TTL");
            Ok(())
        })
    }

    fn remove<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            self.client
                .delete(key)
                .map_err(|e| anyhow!("Memcached DELETE failed: {e}"))?;
            Ok(())
        })
    }

    fn health_check(&self) -> BoxFuture<'_, bool> {
        Box::pin(async move {
            let test_key = "health_check_memcached";
            let test_value = Bytes::from_static(b"health_check");

            match self
                .set_with_ttl(test_key, test_value.clone(), Duration::from_secs(10))
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
        "Memcached"
    }
}
