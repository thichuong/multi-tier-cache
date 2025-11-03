//! L1 Cache - Moka In-Memory Cache
//! 
//! High-performance in-memory cache using Moka for hot data storage.

use std::sync::Arc;
use std::time::{Duration, Instant};
use anyhow::Result;
use serde_json;
use moka::future::Cache;
use std::sync::atomic::{AtomicU64, Ordering};

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

/// L1 Cache using Moka with per-key TTL support
pub struct L1Cache {
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

impl L1Cache {
    /// Create new L1 cache
    pub async fn new() -> Result<Self> {
        println!("  🚀 Initializing L1 Cache (Moka)...");
        
        let cache = Cache::builder()
            .max_capacity(2000) // 2000 entries max
            .time_to_live(Duration::from_secs(3600)) // 1 hour max TTL as safety net
            .time_to_idle(Duration::from_secs(120)) // 2 minutes idle time
            .build();
            
        println!("  ✅ L1 Cache initialized with 2000 capacity, per-key TTL support");
        
        Ok(Self {
            cache,
            hits: Arc::new(AtomicU64::new(0)),
            misses: Arc::new(AtomicU64::new(0)),
            sets: Arc::new(AtomicU64::new(0)),
            coalesced_requests: Arc::new(AtomicU64::new(0)),
        })
    }
    
    /// Get value from L1 cache
    pub async fn get(&self, key: &str) -> Option<serde_json::Value> {
        match self.cache.get(key).await {
            Some(entry) => {
                if entry.is_expired() {
                    // Remove expired entry
                    let _ = self.cache.remove(key).await;
                    self.misses.fetch_add(1, Ordering::Relaxed);
                    None
                } else {
                    self.hits.fetch_add(1, Ordering::Relaxed);
                    Some(entry.value)
                }
            }
            None => {
                self.misses.fetch_add(1, Ordering::Relaxed);
                None
            }
        }
    }
    
    /// Set value with custom TTL (same as set method now)
    pub async fn set_with_ttl(&self, key: &str, value: serde_json::Value, ttl: Duration) -> Result<()> {
        let entry = CacheEntry::new(value, ttl);
        self.cache.insert(key.to_string(), entry).await;
        self.sets.fetch_add(1, Ordering::Relaxed);
        println!("💾 [L1] Cached '{}' with TTL {:?}", key, ttl);
        Ok(())
    }
    
    /// Remove value from cache
    pub async fn remove(&self, key: &str) -> Result<()> {
        self.cache.remove(key).await;
        Ok(())
    }
    
    /// Health check
    pub async fn health_check(&self) -> bool {
        // Test basic functionality with custom TTL
        let test_key = "health_check_l1";
        let test_value = serde_json::json!({"test": true});
        
        match self.set_with_ttl(test_key, test_value.clone(), Duration::from_secs(60)).await {
            Ok(_) => {
                match self.get(test_key).await {
                    Some(retrieved) => {
                        let _ = self.remove(test_key).await;
                        retrieved == test_value
                    }
                    None => false
                }
            }
            Err(_) => false
        }
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
