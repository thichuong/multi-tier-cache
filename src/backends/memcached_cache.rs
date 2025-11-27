//! Memcached Cache - Distributed Cache Backend
//!
//! Memcached-based distributed cache for warm data storage with simple key-value operations.

use anyhow::{anyhow, Result};
use serde_json;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
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
    /// # Example
    ///
    /// ```no_run
    /// # use multi_tier_cache::backends::MemcachedCache;
    /// # async fn example() -> anyhow::Result<()> {
    /// // Set environment variable (optional)
    /// std::env::set_var("MEMCACHED_URL", "memcache://localhost:11211");
    ///
    /// let cache = MemcachedCache::new().await?;
    /// # Ok(())
    /// # }
    /// ```
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
    /// Each tuple contains (`server_address`, stats_map)
    pub fn get_server_stats(
        &self,
    ) -> Result<Vec<(String, std::collections::HashMap<String, String>)>> {
        self.client
            .stats()
            .map_err(|e| anyhow!("Failed to get Memcached stats: {}", e))
    }
}

// ===== Trait Implementations =====

use crate::traits::CacheBackend;
use async_trait::async_trait;

/// Implement CacheBackend trait for MemcachedCache
///
/// This allows MemcachedCache to be used as a pluggable backend in the multi-tier cache system.
#[async_trait]
impl CacheBackend for MemcachedCache {
    async fn get(&self, key: &str) -> Option<serde_json::Value> {
        match self.client.get::<String>(key) {
            Ok(Some(json_str)) => match serde_json::from_str(&json_str) {
                Ok(value) => {
                    self.hits.fetch_add(1, Ordering::Relaxed);
                    Some(value)
                }
                Err(_) => {
                    self.misses.fetch_add(1, Ordering::Relaxed);
                    None
                }
            },
            Ok(None) | Err(_) => {
                self.misses.fetch_add(1, Ordering::Relaxed);
                None
            }
        }
    }

    async fn set_with_ttl(&self, key: &str, value: serde_json::Value, ttl: Duration) -> Result<()> {
        let json_str = serde_json::to_string(&value)?;

        self.client
            .set(key, json_str, ttl.as_secs() as u32)
            .map_err(|e| anyhow!("Memcached SET failed: {}", e))?;

        self.sets.fetch_add(1, Ordering::Relaxed);
        debug!(key = %key, ttl_secs = %ttl.as_secs(), "[Memcached] Cached key with TTL");
        Ok(())
    }

    async fn remove(&self, key: &str) -> Result<()> {
        self.client
            .delete(key)
            .map_err(|e| anyhow!("Memcached DELETE failed: {}", e))?;
        Ok(())
    }

    async fn health_check(&self) -> bool {
        let test_key = "health_check_memcached";
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or(Duration::from_secs(0))
            .as_secs();
        let test_value = serde_json::json!({"test": true, "timestamp": timestamp});

        match self
            .set_with_ttl(test_key, test_value.clone(), Duration::from_secs(10))
            .await
        {
            Ok(_) => match self.get(test_key).await {
                Some(retrieved) => {
                    let _ = self.remove(test_key).await;
                    retrieved["test"].as_bool().unwrap_or(false)
                }
                None => false,
            },
            Err(_) => false,
        }
    }

    fn name(&self) -> &str {
        "Memcached"
    }
}

// Note: MemcachedCache does NOT implement L2CacheBackend trait because
// Memcached does not support TTL introspection (cannot get remaining TTL).
// This means it can be used as L2, but without automatic L2->L1 promotion based on TTL.
