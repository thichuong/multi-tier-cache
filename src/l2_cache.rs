//! L2 Cache - Redis Cache
//!
//! Redis-based distributed cache for warm data storage with persistence.

use std::sync::Arc;
use std::time::Duration;
use anyhow::Result;
use redis::{Client, AsyncCommands};
use redis::aio::ConnectionManager;
use serde_json;
use std::sync::atomic::{AtomicU64, Ordering};

/// L2 Cache using Redis with ConnectionManager for automatic reconnection
pub struct L2Cache {
    /// Redis connection manager - handles reconnection automatically
    conn_manager: ConnectionManager,
    /// Hit counter
    hits: Arc<AtomicU64>,
    /// Miss counter
    misses: Arc<AtomicU64>,
    /// Set counter
    sets: Arc<AtomicU64>,
}

impl L2Cache {
    /// Create new L2 cache with ConnectionManager for automatic reconnection
    pub async fn new() -> Result<Self> {
        println!("  🔴 Initializing L2 Cache (Redis with ConnectionManager)...");

        // Try to connect to Redis
        let redis_url = std::env::var("REDIS_URL")
            .unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());

        let client = Client::open(redis_url.as_str())?;

        // Create ConnectionManager - handles reconnection automatically
        let conn_manager = ConnectionManager::new(client).await?;

        // Test connection
        let mut conn = conn_manager.clone();
        let _: String = redis::cmd("PING").query_async(&mut conn).await?;

        println!("  ✅ L2 Cache connected to Redis at {} (ConnectionManager enabled)", redis_url);

        Ok(Self {
            conn_manager,
            hits: Arc::new(AtomicU64::new(0)),
            misses: Arc::new(AtomicU64::new(0)),
            sets: Arc::new(AtomicU64::new(0)),
        })
    }
    
    /// Get value from L2 cache using persistent ConnectionManager
    pub async fn get(&self, key: &str) -> Option<serde_json::Value> {
        let mut conn = self.conn_manager.clone();

        match conn.get::<_, String>(key).await {
            Ok(json_str) => {
                match serde_json::from_str(&json_str) {
                    Ok(value) => {
                        self.hits.fetch_add(1, Ordering::Relaxed);
                        Some(value)
                    }
                    Err(_) => {
                        self.misses.fetch_add(1, Ordering::Relaxed);
                        None
                    }
                }
            }
            Err(_) => {
                self.misses.fetch_add(1, Ordering::Relaxed);
                None
            }
        }
    }

    /// Get value with its remaining TTL from L2 cache
    ///
    /// Returns tuple of (value, ttl_seconds) if key exists
    /// TTL is in seconds, None if key doesn't exist or has no expiration
    pub async fn get_with_ttl(&self, key: &str) -> Option<(serde_json::Value, Option<Duration>)> {
        let mut conn = self.conn_manager.clone();

        // Get value
        let json_str: String = match conn.get(key).await {
            Ok(s) => s,
            Err(_) => {
                self.misses.fetch_add(1, Ordering::Relaxed);
                return None;
            }
        };

        // Parse JSON
        let value: serde_json::Value = match serde_json::from_str(&json_str) {
            Ok(v) => v,
            Err(_) => {
                self.misses.fetch_add(1, Ordering::Relaxed);
                return None;
            }
        };

        // Get TTL (in seconds, -1 = no expiry, -2 = key doesn't exist)
        let ttl_secs: i64 = match redis::cmd("TTL").arg(key).query_async(&mut conn).await {
            Ok(ttl) => ttl,
            Err(_) => -1, // Fallback: treat as no expiry
        };

        self.hits.fetch_add(1, Ordering::Relaxed);

        let ttl = if ttl_secs > 0 {
            Some(Duration::from_secs(ttl_secs as u64))
        } else {
            None // No expiry or error
        };

        Some((value, ttl))
    }
    
    /// Set value with custom TTL using persistent ConnectionManager
    pub async fn set_with_ttl(&self, key: &str, value: serde_json::Value, ttl: Duration) -> Result<()> {
        let json_str = serde_json::to_string(&value)?;
        let mut conn = self.conn_manager.clone();

        let _: () = conn.set_ex(key, json_str, ttl.as_secs()).await?;
        self.sets.fetch_add(1, Ordering::Relaxed);
        println!("💾 [L2] Cached '{}' with TTL {:?}", key, ttl);
        Ok(())
    }
    
    /// Remove value from cache using persistent ConnectionManager
    pub async fn remove(&self, key: &str) -> Result<()> {
        let mut conn = self.conn_manager.clone();
        let _: () = conn.del(key).await?;
        Ok(())
    }

    /// Scan keys matching a pattern (glob-style: *, ?, [])
    ///
    /// Uses Redis SCAN command (non-blocking, cursor-based iteration)
    /// This is safe for production use, unlike KEYS command.
    ///
    /// # Arguments
    /// * `pattern` - Glob-style pattern (e.g., "user:*", "product:123:*")
    ///
    /// # Returns
    /// Vector of matching key names
    ///
    /// # Examples
    /// ```no_run
    /// # use multi_tier_cache::L2Cache;
    /// # async fn example() -> anyhow::Result<()> {
    /// # let cache = L2Cache::new().await?;
    /// // Find all user cache keys
    /// let keys = cache.scan_keys("user:*").await?;
    ///
    /// // Find specific user's cache keys
    /// let keys = cache.scan_keys("user:123:*").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn scan_keys(&self, pattern: &str) -> Result<Vec<String>> {
        let mut conn = self.conn_manager.clone();
        let mut keys = Vec::new();
        let mut cursor: u64 = 0;

        loop {
            // SCAN cursor MATCH pattern COUNT 100
            let result: (u64, Vec<String>) = redis::cmd("SCAN")
                .arg(cursor)
                .arg("MATCH")
                .arg(pattern)
                .arg("COUNT")
                .arg(100) // Fetch 100 keys per iteration
                .query_async(&mut conn)
                .await?;

            cursor = result.0;
            keys.extend(result.1);

            // Cursor 0 means iteration is complete
            if cursor == 0 {
                break;
            }
        }

        println!("🔍 [L2] Scanned keys matching '{}': {} found", pattern, keys.len());
        Ok(keys)
    }

    /// Remove multiple keys at once (bulk delete)
    ///
    /// More efficient than calling remove() multiple times
    pub async fn remove_bulk(&self, keys: &[String]) -> Result<usize> {
        if keys.is_empty() {
            return Ok(0);
        }

        let mut conn = self.conn_manager.clone();
        let count: usize = conn.del(keys).await?;
        println!("🗑️  [L2] Removed {} keys", count);
        Ok(count)
    }
    
    /// Health check
    pub async fn health_check(&self) -> bool {
        let test_key = "health_check_l2";
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or(Duration::from_secs(0))
            .as_secs();
        let test_value = serde_json::json!({"test": true, "timestamp": timestamp});

        match self.set_with_ttl(test_key, test_value.clone(), Duration::from_secs(10)).await {
            Ok(_) => {
                match self.get(test_key).await {
                    Some(retrieved) => {
                        let _ = self.remove(test_key).await;
                        retrieved["test"].as_bool().unwrap_or(false)
                    }
                    None => false
                }
            }
            Err(_) => false
        }
    }
    
}

// ===== Trait Implementations =====

use crate::traits::{CacheBackend, L2CacheBackend};
use async_trait::async_trait;

/// Implement CacheBackend trait for L2Cache
///
/// This allows L2Cache to be used as a pluggable backend in the multi-tier cache system.
#[async_trait]
impl CacheBackend for L2Cache {
    async fn get(&self, key: &str) -> Option<serde_json::Value> {
        L2Cache::get(self, key).await
    }

    async fn set_with_ttl(
        &self,
        key: &str,
        value: serde_json::Value,
        ttl: Duration,
    ) -> Result<()> {
        L2Cache::set_with_ttl(self, key, value, ttl).await
    }

    async fn remove(&self, key: &str) -> Result<()> {
        L2Cache::remove(self, key).await
    }

    async fn health_check(&self) -> bool {
        L2Cache::health_check(self).await
    }

    fn name(&self) -> &str {
        "Redis (L2)"
    }
}

/// Implement L2CacheBackend trait for L2Cache
///
/// This extends CacheBackend with TTL introspection capabilities needed for L2->L1 promotion.
#[async_trait]
impl L2CacheBackend for L2Cache {
    async fn get_with_ttl(
        &self,
        key: &str,
    ) -> Option<(serde_json::Value, Option<Duration>)> {
        L2Cache::get_with_ttl(self, key).await
    }
}
