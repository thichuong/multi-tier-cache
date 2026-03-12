use crate::traits::{CacheBackend, L2CacheBackend};
use anyhow::{Context, Result};
use bytes::Bytes;
use futures_util::future::BoxFuture;
use redis::aio::ConnectionManager;
use redis::{AsyncCommands, Client};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info};

/// Redis distributed cache with `ConnectionManager` for automatic reconnection
pub struct RedisCache {
    /// Redis connection manager
    conn_manager: ConnectionManager,
    /// Hit counter
    hits: Arc<AtomicU64>,
    /// Miss counter
    misses: Arc<AtomicU64>,
    /// Set counter
    sets: Arc<AtomicU64>,
}

impl RedisCache {
    /// Create new Redis cache
    pub async fn new() -> Result<Self> {
        let redis_url =
            std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());
        Self::with_url(&redis_url).await
    }

    /// Create new Redis cache with custom URL
    pub async fn with_url(redis_url: &str) -> Result<Self> {
        info!(redis_url = %redis_url, "Initializing Redis Cache with ConnectionManager");

        let client = Client::open(redis_url)
            .with_context(|| format!("Failed to create Redis client with URL: {redis_url}"))?;

        let conn_manager = ConnectionManager::new(client)
            .await
            .context("Failed to establish Redis connection manager")?;

        let mut conn = conn_manager.clone();
        let _: String = redis::cmd("PING")
            .query_async(&mut conn)
            .await
            .context("Redis PING health check failed")?;

        info!(redis_url = %redis_url, "Redis Cache connected successfully");

        Ok(Self {
            conn_manager,
            hits: Arc::new(AtomicU64::new(0)),
            misses: Arc::new(AtomicU64::new(0)),
            sets: Arc::new(AtomicU64::new(0)),
        })
    }

    /// Scan keys matching a pattern
    pub async fn scan_keys(&self, pattern: &str) -> Result<Vec<String>> {
        let mut conn = self.conn_manager.clone();
        let mut keys = Vec::new();
        let mut cursor: u64 = 0;

        loop {
            let result: (u64, Vec<String>) = redis::cmd("SCAN")
                .arg(cursor)
                .arg("MATCH")
                .arg(pattern)
                .arg("COUNT")
                .arg(100)
                .query_async(&mut conn)
                .await?;

            cursor = result.0;
            keys.extend(result.1);

            if cursor == 0 {
                break;
            }
        }

        debug!(pattern = %pattern, count = keys.len(), "[Redis] Scanned keys matching pattern");
        Ok(keys)
    }

    /// Remove multiple keys at once
    pub async fn remove_bulk(&self, keys: &[String]) -> Result<usize> {
        if keys.is_empty() {
            return Ok(0);
        }

        let mut conn = self.conn_manager.clone();
        let count: usize = conn.del(keys).await?;
        debug!(count = count, "[Redis] Removed keys in bulk");
        Ok(count)
    }
}

// ===== Trait Implementations =====


/// Implement `CacheBackend` trait for `RedisCache`
impl CacheBackend for RedisCache {
    fn get<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Option<Bytes>> {
        Box::pin(async move {
            let mut conn = self.conn_manager.clone();
            let result: redis::RedisResult<Vec<u8>> = conn.get(key).await;
            match result {
                Ok(bytes) if !bytes.is_empty() => {
                    self.hits.fetch_add(1, Ordering::Relaxed);
                    Some(Bytes::from(bytes))
                }
                _ => {
                    self.misses.fetch_add(1, Ordering::Relaxed);
                    None
                }
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
            let mut conn = self.conn_manager.clone();
            let result = conn.pset_ex(key, value.to_vec(), ttl.as_millis() as u64).await;
            if result.is_ok() {
                self.sets.fetch_add(1, Ordering::Relaxed);
                debug!(key = %key, ttl_ms = %ttl.as_millis(), "[Redis] Cached key bytes with TTL");
            }
            result.map_err(|e| anyhow::anyhow!("Redis set failed: {}", e))
        })
    }

    fn remove<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let mut conn = self.conn_manager.clone();
            conn.del(key)
                .await
                .map_err(|e| anyhow::anyhow!("Redis del failed: {}", e))
        })
    }

    fn health_check(&self) -> BoxFuture<'_, bool> {
        Box::pin(async move {
            let mut conn = self.conn_manager.clone();
            let result: redis::RedisResult<String> = redis::cmd("PING").query_async(&mut conn).await;
            result.is_ok()
        })
    }

    fn remove_pattern<'a>(&'a self, pattern: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let keys = self.scan_keys(pattern).await?;
            if !keys.is_empty() {
                self.remove_bulk(&keys).await?;
            }
            Ok(())
        })
    }

    fn name(&self) -> &'static str {
        "Redis"
    }
}

impl L2CacheBackend for RedisCache {
    fn get_with_ttl<'a>(
        &'a self,
        key: &'a str,
    ) -> BoxFuture<'a, Option<(Bytes, Option<Duration>)>> {
        Box::pin(async move {
            let mut conn = self.conn_manager.clone();
            // Pipelining is better:
            let (bytes, ttl_secs): (Option<Vec<u8>>, i64) = match redis::pipe()
                .get(key)
                .ttl(key)
                .query_async(&mut conn)
                .await
            {
                Ok(res) => res,
                Err(_) => return None,
            };

            match bytes {
                Some(b) if !b.is_empty() => {
                    self.hits.fetch_add(1, Ordering::Relaxed);
                    let ttl = if ttl_secs > 0 {
                        Some(Duration::from_secs(ttl_secs.unsigned_abs()))
                    } else {
                        None
                    };
                    Some((Bytes::from(b), ttl))
                }
                _ => {
                    self.misses.fetch_add(1, Ordering::Relaxed);
                    None
                }
            }
        })
    }
}
