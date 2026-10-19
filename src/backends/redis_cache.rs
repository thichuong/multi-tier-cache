use crate::error::CacheResult;
use crate::traits::{CacheBackend, L2CacheBackend};
use bytes::Bytes;
use futures_util::future::BoxFuture;
use redis::aio::ConnectionManager;
use redis::{AsyncCommands, Client};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
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
    ///
    /// # Errors
    ///
    /// Returns an error if the Redis URL is invalid or connection fails.
    pub async fn new() -> CacheResult<Self> {
        let redis_url =
            std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());
        Self::with_url(&redis_url).await
    }

    /// Create new Redis cache with custom URL
    ///
    /// # Errors
    ///
    /// Returns an error if the Redis URL is invalid or connection fails.
    pub async fn with_url(redis_url: &str) -> CacheResult<Self> {
        info!(redis_url = %redis_url, "Initializing Redis Cache with ConnectionManager");

        let client = Client::open(redis_url).map_err(|e| {
            crate::error::CacheError::ConfigError(format!("Failed to create Redis client: {e}"))
        })?;

        let conn_manager = ConnectionManager::new(client).await.map_err(|e| {
            crate::error::CacheError::BackendError(format!(
                "Failed to establish Redis connection manager: {e}"
            ))
        })?;

        let mut conn = conn_manager.clone();
        let _: String = redis::cmd("PING")
            .query_async(&mut conn)
            .await
            .map_err(|e| {
                crate::error::CacheError::BackendError(format!(
                    "Redis PING health check failed: {e}"
                ))
            })?;

        info!(redis_url = %redis_url, "Redis Cache connected successfully");

        Ok(Self {
            conn_manager,
            hits: Arc::new(AtomicU64::new(0)),
            misses: Arc::new(AtomicU64::new(0)),
            sets: Arc::new(AtomicU64::new(0)),
        })
    }

    /// Scan keys matching a pattern
    ///
    /// # Errors
    ///
    /// Returns an error if the SCAN command fails.
    pub async fn scan_keys(&self, pattern: &str) -> CacheResult<Vec<String>> {
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
    ///
    /// # Errors
    ///
    /// Returns an error if the DEL command fails.
    pub async fn remove_bulk(&self, keys: &[String]) -> CacheResult<usize> {
        if keys.is_empty() {
            return Ok(0);
        }

        let mut conn = self.conn_manager.clone();
        let count: usize = conn.del(keys).await?;
        debug!(count = count, "[Redis] Removed keys in bulk");
        Ok(count)
    }

    /// Get a clone of the internal connection manager
    pub(crate) fn connection_manager(&self) -> ConnectionManager {
        self.conn_manager.clone()
    }
}

// ===== Trait Implementations =====

/// Implement `CacheBackend` trait for `RedisCache`
impl CacheBackend for RedisCache {
    fn get<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Option<Bytes>> {
        Box::pin(async move {
            let mut conn = self.conn_manager.clone();
            let result: redis::RedisResult<Option<Vec<u8>>> = conn.get(key).await;
            if let Ok(Some(bytes)) = result {
                self.hits.fetch_add(1, Ordering::Relaxed);
                Some(Bytes::from(bytes))
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
    ) -> BoxFuture<'a, CacheResult<()>> {
        Box::pin(async move {
            let mut conn = self.conn_manager.clone();
            let ttl_ms = u64::try_from(ttl.as_millis()).unwrap_or(u64::MAX).max(1);
            let result = conn.pset_ex(key, value.as_ref(), ttl_ms).await;
            if result.is_ok() {
                self.sets.fetch_add(1, Ordering::Relaxed);
                debug!(key = %key, ttl_ms = %ttl.as_millis(), "[Redis] Cached key bytes with TTL");
            }
            result.map_err(|e| {
                crate::error::CacheError::BackendError(format!("Redis set failed: {e}"))
            })
        })
    }

    fn remove<'a>(&'a self, key: &'a str) -> BoxFuture<'a, CacheResult<()>> {
        Box::pin(async move {
            let mut conn = self.conn_manager.clone();
            let _: usize = conn.del(key).await?;
            Ok(())
        })
    }

    fn health_check(&self) -> BoxFuture<'_, bool> {
        Box::pin(async move {
            let mut conn = self.conn_manager.clone();
            let result: redis::RedisResult<String> =
                redis::cmd("PING").query_async(&mut conn).await;
            result.is_ok()
        })
    }

    fn remove_pattern<'a>(&'a self, pattern: &'a str) -> BoxFuture<'a, CacheResult<()>> {
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
            // Pipelining with PTTL for millisecond precision
            let mut pipe = redis::pipe();
            pipe.get(key).cmd("PTTL").arg(key);
            let (bytes, ttl_ms): (Option<Vec<u8>>, i64) = match pipe.query_async(&mut conn).await {
                Ok(res) => res,
                Err(_) => return None,
            };

            if let Some(b) = bytes {
                self.hits.fetch_add(1, Ordering::Relaxed);
                let ttl = if ttl_ms > 0 {
                    Some(Duration::from_millis(ttl_ms.unsigned_abs()))
                } else {
                    None
                };
                Some((Bytes::from(b), ttl))
            } else {
                self.misses.fetch_add(1, Ordering::Relaxed);
                None
            }
        })
    }
}
