//! Cache Manager - Unified Cache Operations
//!
//! Manages operations across L1 (Moka) and L2 (Redis) caches with intelligent fallback.

use std::sync::Arc;
use std::time::Duration;
use std::future::Future;
use anyhow::Result;
use serde_json;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use dashmap::DashMap;
use tokio::sync::Mutex;

use crate::backends::{L1Cache, L2Cache};
use crate::traits::{CacheBackend, L2CacheBackend, StreamingBackend};
use super::invalidation::{
    InvalidationConfig, InvalidationPublisher, InvalidationSubscriber,
    InvalidationMessage, AtomicInvalidationStats, InvalidationStats,
};

/// RAII cleanup guard for in-flight request tracking
/// Ensures that entries are removed from DashMap even on early return or panic
struct CleanupGuard<'a> {
    map: &'a DashMap<String, Arc<Mutex<()>>>,
    key: String,
}

impl<'a> Drop for CleanupGuard<'a> {
    fn drop(&mut self) {
        self.map.remove(&self.key);
    }
}

/// Cache strategies for different data types
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum CacheStrategy {
    /// Real-time data - 10 seconds TTL
    RealTime,
    /// Short-term data - 5 minutes TTL  
    ShortTerm,
    /// Medium-term data - 1 hour TTL
    MediumTerm,
    /// Long-term data - 3 hours TTL
    LongTerm,
    /// Custom TTL
    Custom(Duration),
    /// Default strategy (5 minutes)
    Default,
}

impl CacheStrategy {
    /// Convert strategy to duration
    pub fn to_duration(&self) -> Duration {
        match self {
            Self::RealTime => Duration::from_secs(10),
            Self::ShortTerm => Duration::from_secs(300), // 5 minutes
            Self::MediumTerm => Duration::from_secs(3600), // 1 hour
            Self::LongTerm => Duration::from_secs(10800), // 3 hours
            Self::Custom(duration) => *duration,
            Self::Default => Duration::from_secs(300),
        }
    }
}

/// Statistics for a single cache tier
#[derive(Debug)]
pub struct TierStats {
    /// Tier level (1 = L1, 2 = L2, 3 = L3, etc.)
    pub tier_level: usize,
    /// Number of cache hits at this tier
    pub hits: AtomicU64,
    /// Backend name for identification
    pub backend_name: String,
}

impl Clone for TierStats {
    fn clone(&self) -> Self {
        Self {
            tier_level: self.tier_level,
            hits: AtomicU64::new(self.hits.load(Ordering::Relaxed)),
            backend_name: self.backend_name.clone(),
        }
    }
}

impl TierStats {
    fn new(tier_level: usize, backend_name: String) -> Self {
        Self {
            tier_level,
            hits: AtomicU64::new(0),
            backend_name,
        }
    }

    /// Get current hit count
    pub fn hit_count(&self) -> u64 {
        self.hits.load(Ordering::Relaxed)
    }
}

/// A single cache tier in the multi-tier architecture
pub struct CacheTier {
    /// Cache backend for this tier
    backend: Arc<dyn L2CacheBackend>,
    /// Tier level (1 = hottest/fastest, higher = colder/slower)
    tier_level: usize,
    /// Enable automatic promotion to upper tiers on cache hit
    promotion_enabled: bool,
    /// TTL scale factor (multiplier for TTL when storing/promoting)
    ttl_scale: f64,
    /// Statistics for this tier
    stats: TierStats,
}

impl CacheTier {
    /// Create a new cache tier
    pub fn new(
        backend: Arc<dyn L2CacheBackend>,
        tier_level: usize,
        promotion_enabled: bool,
        ttl_scale: f64,
    ) -> Self {
        let backend_name = backend.name().to_string();
        Self {
            backend,
            tier_level,
            promotion_enabled,
            ttl_scale,
            stats: TierStats::new(tier_level, backend_name),
        }
    }

    /// Get value with TTL from this tier
    async fn get_with_ttl(&self, key: &str) -> Option<(serde_json::Value, Option<Duration>)> {
        self.backend.get_with_ttl(key).await
    }

    /// Set value with TTL in this tier
    async fn set_with_ttl(&self, key: &str, value: serde_json::Value, ttl: Duration) -> Result<()> {
        let scaled_ttl = Duration::from_secs_f64(ttl.as_secs_f64() * self.ttl_scale);
        self.backend.set_with_ttl(key, value, scaled_ttl).await
    }

    /// Remove value from this tier
    async fn remove(&self, key: &str) -> Result<()> {
        self.backend.remove(key).await
    }

    /// Record a cache hit for this tier
    fn record_hit(&self) {
        self.stats.hits.fetch_add(1, Ordering::Relaxed);
    }
}

/// Configuration for a cache tier (used in builder pattern)
#[derive(Debug, Clone)]
pub struct TierConfig {
    /// Tier level (1, 2, 3, 4...)
    pub tier_level: usize,
    /// Enable promotion to upper tiers on hit
    pub promotion_enabled: bool,
    /// TTL scale factor (1.0 = same as base TTL)
    pub ttl_scale: f64,
}

impl TierConfig {
    /// Create new tier configuration
    pub fn new(tier_level: usize) -> Self {
        Self {
            tier_level,
            promotion_enabled: true,
            ttl_scale: 1.0,
        }
    }

    /// Configure as L1 (hot tier)
    pub fn as_l1() -> Self {
        Self {
            tier_level: 1,
            promotion_enabled: false, // L1 is already top tier
            ttl_scale: 1.0,
        }
    }

    /// Configure as L2 (warm tier)
    pub fn as_l2() -> Self {
        Self {
            tier_level: 2,
            promotion_enabled: true,
            ttl_scale: 1.0,
        }
    }

    /// Configure as L3 (cold tier) with longer TTL
    pub fn as_l3() -> Self {
        Self {
            tier_level: 3,
            promotion_enabled: true,
            ttl_scale: 2.0, // Keep data 2x longer
        }
    }

    /// Configure as L4 (archive tier) with much longer TTL
    pub fn as_l4() -> Self {
        Self {
            tier_level: 4,
            promotion_enabled: true,
            ttl_scale: 8.0, // Keep data 8x longer
        }
    }

    /// Set promotion enabled
    pub fn with_promotion(mut self, enabled: bool) -> Self {
        self.promotion_enabled = enabled;
        self
    }

    /// Set TTL scale factor
    pub fn with_ttl_scale(mut self, scale: f64) -> Self {
        self.ttl_scale = scale;
        self
    }

    /// Set tier level
    pub fn with_level(mut self, level: usize) -> Self {
        self.tier_level = level;
        self
    }
}

/// Proxy wrapper to convert L2CacheBackend to CacheBackend
/// (Rust doesn't support automatic trait upcasting for trait objects)
struct ProxyCacheBackend {
    backend: Arc<dyn L2CacheBackend>,
}

#[async_trait::async_trait]
impl CacheBackend for ProxyCacheBackend {
    async fn get(&self, key: &str) -> Option<serde_json::Value> {
        self.backend.get(key).await
    }

    async fn set_with_ttl(&self, key: &str, value: serde_json::Value, ttl: Duration) -> Result<()> {
        self.backend.set_with_ttl(key, value, ttl).await
    }

    async fn remove(&self, key: &str) -> Result<()> {
        self.backend.remove(key).await
    }

    async fn health_check(&self) -> bool {
        self.backend.health_check().await
    }

    fn name(&self) -> &str {
        self.backend.name()
    }
}

/// Cache Manager - Unified operations across multiple cache tiers
///
/// Supports both legacy 2-tier (L1+L2) and new multi-tier (L1+L2+L3+L4+...) architectures.
/// When `tiers` is Some, it uses the dynamic multi-tier system. Otherwise, falls back to
/// legacy L1+L2 behavior for backward compatibility.
pub struct CacheManager {
    /// Dynamic multi-tier cache architecture (v0.5.0+)
    /// If Some, this takes precedence over l1_cache/l2_cache fields
    tiers: Option<Vec<CacheTier>>,

    // ===== Legacy fields (v0.1.0 - v0.4.x) =====
    // Maintained for backward compatibility
    /// L1 Cache (trait object for pluggable backends)
    l1_cache: Arc<dyn CacheBackend>,
    /// L2 Cache (trait object for pluggable backends)
    l2_cache: Arc<dyn L2CacheBackend>,
    /// L2 Cache concrete instance (for invalidation scan_keys)
    l2_cache_concrete: Option<Arc<L2Cache>>,

    /// Optional streaming backend (defaults to L2 if it implements StreamingBackend)
    streaming_backend: Option<Arc<dyn StreamingBackend>>,
    /// Statistics (AtomicU64 is already thread-safe, no Arc needed)
    total_requests: AtomicU64,
    l1_hits: AtomicU64,
    l2_hits: AtomicU64,
    misses: AtomicU64,
    promotions: AtomicUsize,
    /// In-flight requests to prevent Cache Stampede on L2/compute operations
    in_flight_requests: Arc<DashMap<String, Arc<Mutex<()>>>>,
    /// Invalidation publisher (for broadcasting invalidation messages)
    invalidation_publisher: Option<Arc<Mutex<InvalidationPublisher>>>,
    /// Invalidation subscriber (for receiving invalidation messages)
    invalidation_subscriber: Option<Arc<InvalidationSubscriber>>,
    /// Invalidation statistics
    invalidation_stats: Arc<AtomicInvalidationStats>,
}

impl CacheManager {
    /// Create new cache manager with trait objects (pluggable backends)
    ///
    /// This is the primary constructor for v0.3.0+, supporting custom cache backends.
    ///
    /// # Arguments
    ///
    /// * `l1_cache` - Any L1 cache backend implementing `CacheBackend` trait
    /// * `l2_cache` - Any L2 cache backend implementing `L2CacheBackend` trait
    /// * `streaming_backend` - Optional streaming backend (None to disable streaming)
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use multi_tier_cache::{CacheManager, L1Cache, L2Cache};
    /// use std::sync::Arc;
    ///
    /// let l1: Arc<dyn CacheBackend> = Arc::new(L1Cache::new().await?);
    /// let l2: Arc<dyn L2CacheBackend> = Arc::new(L2Cache::new().await?);
    ///
    /// let manager = CacheManager::new_with_backends(l1, l2, None).await?;
    /// ```
    pub async fn new_with_backends(
        l1_cache: Arc<dyn CacheBackend>,
        l2_cache: Arc<dyn L2CacheBackend>,
        streaming_backend: Option<Arc<dyn StreamingBackend>>,
    ) -> Result<Self> {
        println!("  🎯 Initializing Cache Manager with custom backends...");
        println!("    L1: {}", l1_cache.name());
        println!("    L2: {}", l2_cache.name());
        if streaming_backend.is_some() {
            println!("    Streaming: enabled");
        } else {
            println!("    Streaming: disabled");
        }

        Ok(Self {
            tiers: None, // Legacy mode: use l1_cache/l2_cache fields
            l1_cache,
            l2_cache,
            l2_cache_concrete: None,
            streaming_backend,
            total_requests: AtomicU64::new(0),
            l1_hits: AtomicU64::new(0),
            l2_hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            promotions: AtomicUsize::new(0),
            in_flight_requests: Arc::new(DashMap::new()),
            invalidation_publisher: None,
            invalidation_subscriber: None,
            invalidation_stats: Arc::new(AtomicInvalidationStats::default()),
        })
    }

    /// Create new cache manager with default backends (backward compatible)
    ///
    /// This is the legacy constructor maintained for backward compatibility.
    /// New code should prefer `new_with_backends()` or `CacheSystemBuilder`.
    ///
    /// # Arguments
    ///
    /// * `l1_cache` - Moka L1 cache instance
    /// * `l2_cache` - Redis L2 cache instance
    pub async fn new(l1_cache: Arc<L1Cache>, l2_cache: Arc<L2Cache>) -> Result<Self> {
        println!("  🎯 Initializing Cache Manager...");

        // Convert concrete types to trait objects
        let l1_backend: Arc<dyn CacheBackend> = l1_cache.clone();
        let l2_backend: Arc<dyn L2CacheBackend> = l2_cache.clone();

        // Create RedisStreams backend for streaming functionality
        let redis_url = std::env::var("REDIS_URL")
            .unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());
        let redis_streams = crate::redis_streams::RedisStreams::new(&redis_url).await?;
        let streaming_backend: Arc<dyn StreamingBackend> = Arc::new(redis_streams);

        Self::new_with_backends(l1_backend, l2_backend, Some(streaming_backend)).await
    }

    /// Create new cache manager with invalidation support
    ///
    /// This constructor enables cross-instance cache invalidation via Redis Pub/Sub.
    ///
    /// # Arguments
    ///
    /// * `l1_cache` - Moka L1 cache instance
    /// * `l2_cache` - Redis L2 cache instance
    /// * `redis_url` - Redis connection URL for Pub/Sub
    /// * `config` - Invalidation configuration
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use multi_tier_cache::{CacheManager, L1Cache, L2Cache, InvalidationConfig};
    ///
    /// let config = InvalidationConfig {
    ///     channel: "my_app:cache:invalidate".to_string(),
    ///     ..Default::default()
    /// };
    ///
    /// let manager = CacheManager::new_with_invalidation(
    ///     l1, l2, "redis://localhost", config
    /// ).await?;
    /// ```
    pub async fn new_with_invalidation(
        l1_cache: Arc<L1Cache>,
        l2_cache: Arc<L2Cache>,
        redis_url: &str,
        config: InvalidationConfig,
    ) -> Result<Self> {
        println!("  🎯 Initializing Cache Manager with Invalidation...");
        println!("    Pub/Sub channel: {}", config.channel);

        // Convert concrete types to trait objects
        let l1_backend: Arc<dyn CacheBackend> = l1_cache.clone();
        let l2_backend: Arc<dyn L2CacheBackend> = l2_cache.clone();

        // Create RedisStreams backend for streaming functionality
        let redis_streams = crate::redis_streams::RedisStreams::new(redis_url).await?;
        let streaming_backend: Arc<dyn StreamingBackend> = Arc::new(redis_streams);

        // Create publisher
        let client = redis::Client::open(redis_url)?;
        let conn_manager = redis::aio::ConnectionManager::new(client).await?;
        let publisher = InvalidationPublisher::new(conn_manager, config.clone());

        // Create subscriber
        let subscriber = InvalidationSubscriber::new(redis_url, config.clone())?;
        let invalidation_stats = Arc::new(AtomicInvalidationStats::default());

        let manager = Self {
            tiers: None, // Legacy mode: use l1_cache/l2_cache fields
            l1_cache: l1_backend,
            l2_cache: l2_backend,
            l2_cache_concrete: Some(l2_cache),
            streaming_backend: Some(streaming_backend),
            total_requests: AtomicU64::new(0),
            l1_hits: AtomicU64::new(0),
            l2_hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            promotions: AtomicUsize::new(0),
            in_flight_requests: Arc::new(DashMap::new()),
            invalidation_publisher: Some(Arc::new(Mutex::new(publisher))),
            invalidation_subscriber: Some(Arc::new(subscriber)),
            invalidation_stats,
        };

        // Start subscriber with handler
        manager.start_invalidation_subscriber();

        println!("  ✅ Cache Manager initialized with invalidation support");

        Ok(manager)
    }

    /// Create new cache manager with multi-tier architecture (v0.5.0+)
    ///
    /// This constructor enables dynamic multi-tier caching with 3, 4, or more tiers.
    /// Tiers are checked in order (lower tier_level = faster/hotter).
    ///
    /// # Arguments
    ///
    /// * `tiers` - Vector of configured cache tiers (must be sorted by tier_level ascending)
    /// * `streaming_backend` - Optional streaming backend
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use multi_tier_cache::{CacheManager, CacheTier, TierConfig, L1Cache, L2Cache};
    /// use std::sync::Arc;
    ///
    /// // L1 + L2 + L3 setup
    /// let l1 = Arc::new(L1Cache::new().await?);
    /// let l2 = Arc::new(L2Cache::new().await?);
    /// let l3 = Arc::new(RocksDBCache::new("/tmp/cache").await?);
    ///
    /// let tiers = vec![
    ///     CacheTier::new(l1, 1, false, 1.0),  // L1 - no promotion
    ///     CacheTier::new(l2, 2, true, 1.0),   // L2 - promote to L1
    ///     CacheTier::new(l3, 3, true, 2.0),   // L3 - promote to L2&L1, 2x TTL
    /// ];
    ///
    /// let manager = CacheManager::new_with_tiers(tiers, None).await?;
    /// ```
    pub async fn new_with_tiers(
        tiers: Vec<CacheTier>,
        streaming_backend: Option<Arc<dyn StreamingBackend>>,
    ) -> Result<Self> {
        println!("  🎯 Initializing Multi-Tier Cache Manager...");
        println!("    Tier count: {}", tiers.len());
        for tier in &tiers {
            println!(
                "    L{}: {} (promotion={}, ttl_scale={})",
                tier.tier_level,
                tier.stats.backend_name,
                tier.promotion_enabled,
                tier.ttl_scale
            );
        }

        // Validate tiers are sorted by level
        for i in 1..tiers.len() {
            if tiers[i].tier_level <= tiers[i - 1].tier_level {
                anyhow::bail!(
                    "Tiers must be sorted by tier_level ascending (found L{} after L{})",
                    tiers[i].tier_level,
                    tiers[i - 1].tier_level
                );
            }
        }

        // For backward compatibility with legacy code, we need dummy l1/l2 caches
        // Use first tier as l1, second tier as l2 if available
        let (l1_cache, l2_cache) = if tiers.len() >= 2 {
            (tiers[0].backend.clone(), tiers[1].backend.clone())
        } else if tiers.len() == 1 {
            // Only one tier - use it for both
            let tier = tiers[0].backend.clone();
            (tier.clone(), tier)
        } else {
            anyhow::bail!("At least one cache tier is required");
        };

        // Convert to CacheBackend trait for l1 (L2CacheBackend extends CacheBackend)
        let l1_backend: Arc<dyn CacheBackend> = Arc::new(ProxyCacheBackend {
            backend: l1_cache.clone(),
        });

        Ok(Self {
            tiers: Some(tiers),
            l1_cache: l1_backend,
            l2_cache,
            l2_cache_concrete: None,
            streaming_backend,
            total_requests: AtomicU64::new(0),
            l1_hits: AtomicU64::new(0),
            l2_hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            promotions: AtomicUsize::new(0),
            in_flight_requests: Arc::new(DashMap::new()),
            invalidation_publisher: None,
            invalidation_subscriber: None,
            invalidation_stats: Arc::new(AtomicInvalidationStats::default()),
        })
    }

    /// Start the invalidation subscriber background task
    fn start_invalidation_subscriber(&self) {
        if let Some(subscriber) = &self.invalidation_subscriber {
            let l1_cache = Arc::clone(&self.l1_cache);
            let l2_cache_concrete = self.l2_cache_concrete.clone();

            subscriber.start(move |msg| {
                let l1 = Arc::clone(&l1_cache);
                let _l2 = l2_cache_concrete.clone();

                async move {
                    match msg {
                        InvalidationMessage::Remove { key } => {
                            // Remove from L1
                            l1.remove(&key).await?;
                            println!("🗑️  [Invalidation] Removed '{}' from L1", key);
                        }
                        InvalidationMessage::Update { key, value, ttl_secs } => {
                            // Update L1 with new value
                            let ttl = ttl_secs
                                .map(Duration::from_secs)
                                .unwrap_or_else(|| Duration::from_secs(300));
                            l1.set_with_ttl(&key, value, ttl).await?;
                            println!("🔄 [Invalidation] Updated '{}' in L1", key);
                        }
                        InvalidationMessage::RemovePattern { pattern } => {
                            // For pattern-based invalidation, we can't easily iterate L1 cache
                            // So we just log it. The pattern invalidation is mainly for L2.
                            // L1 entries will naturally expire via TTL.
                            println!("🔍 [Invalidation] Pattern '{}' invalidated (L1 will expire naturally)", pattern);
                        }
                        InvalidationMessage::RemoveBulk { keys } => {
                            // Remove multiple keys from L1
                            for key in keys {
                                if let Err(e) = l1.remove(&key).await {
                                    eprintln!("⚠️  Failed to remove '{}' from L1: {}", key, e);
                                }
                            }
                            println!("🗑️  [Invalidation] Bulk removed keys from L1");
                        }
                    }
                    Ok(())
                }
            });

            println!("📡 Invalidation subscriber started");
        }
    }

    /// Get value from cache using multi-tier architecture (v0.5.0+)
    ///
    /// This method iterates through all configured tiers and automatically promotes
    /// to upper tiers on cache hit.
    async fn get_multi_tier(&self, key: &str) -> Result<Option<serde_json::Value>> {
        let tiers = self.tiers.as_ref().unwrap(); // Safe: only called when tiers is Some

        // Try each tier sequentially (sorted by tier_level)
        for (tier_index, tier) in tiers.iter().enumerate() {
            if let Some((value, ttl)) = tier.get_with_ttl(key).await {
                // Cache hit!
                tier.record_hit();

                // Promote to all upper tiers (if promotion enabled)
                if tier.promotion_enabled && tier_index > 0 {
                    let promotion_ttl = ttl.unwrap_or_else(|| CacheStrategy::Default.to_duration());

                    // Promote to all tiers above this one
                    for upper_tier in tiers[..tier_index].iter().rev() {
                        if let Err(e) = upper_tier.set_with_ttl(key, value.clone(), promotion_ttl).await {
                            eprintln!(
                                "⚠️  Failed to promote '{}' from L{} to L{}: {}",
                                key, tier.tier_level, upper_tier.tier_level, e
                            );
                        } else {
                            self.promotions.fetch_add(1, Ordering::Relaxed);
                            println!(
                                "⬆️  Promoted '{}' from L{} to L{} (TTL: {:?})",
                                key, tier.tier_level, upper_tier.tier_level, promotion_ttl
                            );
                        }
                    }
                }

                return Ok(Some(value));
            }
        }

        // Cache miss across all tiers
        Ok(None)
    }

    /// Get value from cache (L1 first, then L2 fallback with promotion)
    ///
    /// This method now includes built-in Cache Stampede protection when cache misses occur.
    /// Multiple concurrent requests for the same missing key will be coalesced to prevent
    /// unnecessary duplicate work on external data sources.
    ///
    /// Supports both legacy 2-tier mode and new multi-tier mode (v0.5.0+).
    ///
    /// # Arguments
    /// * `key` - Cache key to retrieve
    ///
    /// # Returns
    /// * `Ok(Some(value))` - Cache hit, value found in any tier
    /// * `Ok(None)` - Cache miss, value not found in any cache
    /// * `Err(error)` - Cache operation failed
    pub async fn get(&self, key: &str) -> Result<Option<serde_json::Value>> {
        self.total_requests.fetch_add(1, Ordering::Relaxed);

        // NEW: Multi-tier mode (v0.5.0+)
        if self.tiers.is_some() {
            // Fast path for L1 (first tier) - no locking needed
            if let Some(tier1) = self.tiers.as_ref().unwrap().first() {
                if let Some((value, _ttl)) = tier1.get_with_ttl(key).await {
                    tier1.record_hit();
                    // Update legacy stats for backward compatibility
                    self.l1_hits.fetch_add(1, Ordering::Relaxed);
                    return Ok(Some(value));
                }
            }

            // L1 miss - use stampede protection for lower tiers
            let key_owned = key.to_string();
            let lock_guard = self.in_flight_requests
                .entry(key_owned.clone())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone();

            let _guard = lock_guard.lock().await;
            let cleanup_guard = CleanupGuard {
                map: &self.in_flight_requests,
                key: key_owned.clone(),
            };

            // Double-check L1 after acquiring lock
            if let Some(tier1) = self.tiers.as_ref().unwrap().first() {
                if let Some((value, _ttl)) = tier1.get_with_ttl(key).await {
                    tier1.record_hit();
                    self.l1_hits.fetch_add(1, Ordering::Relaxed);
                    return Ok(Some(value));
                }
            }

            // Check remaining tiers with promotion
            let result = self.get_multi_tier(key).await?;

            if result.is_some() {
                // Hit in L2+ tier - update legacy stats
                if self.tiers.as_ref().unwrap().len() >= 2 {
                    self.l2_hits.fetch_add(1, Ordering::Relaxed);
                }
            } else {
                self.misses.fetch_add(1, Ordering::Relaxed);
            }

            drop(cleanup_guard);
            return Ok(result);
        }

        // LEGACY: 2-tier mode (L1 + L2)
        // Fast path: Try L1 first (no locking needed)
        if let Some(value) = self.l1_cache.get(key).await {
            self.l1_hits.fetch_add(1, Ordering::Relaxed);
            return Ok(Some(value));
        }
        
        // L1 miss - implement Cache Stampede protection for L2 lookup
        let key_owned = key.to_string();
        let lock_guard = self.in_flight_requests
            .entry(key_owned.clone())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone();
        
        let _guard = lock_guard.lock().await;
        
        // RAII cleanup guard - ensures entry is removed even on early return or panic
        let cleanup_guard = CleanupGuard {
            map: &self.in_flight_requests,
            key: key_owned.clone(),
        };
        
        // Double-check L1 cache after acquiring lock
        // (Another concurrent request might have populated it while we were waiting)
        if let Some(value) = self.l1_cache.get(key).await {
            self.l1_hits.fetch_add(1, Ordering::Relaxed);
            // cleanup_guard will auto-remove entry on drop
            return Ok(Some(value));
        }
        
        // Check L2 cache with TTL information
        if let Some((value, ttl)) = self.l2_cache.get_with_ttl(key).await {
            self.l2_hits.fetch_add(1, Ordering::Relaxed);

            // Promote to L1 with same TTL as Redis (or default if no TTL)
            let promotion_ttl = ttl.unwrap_or_else(|| CacheStrategy::Default.to_duration());

            if let Err(_) = self.l1_cache.set_with_ttl(key, value.clone(), promotion_ttl).await {
                // L1 promotion failed, but we still have the data
                eprintln!("⚠️ Failed to promote key '{}' to L1 cache", key);
            } else {
                self.promotions.fetch_add(1, Ordering::Relaxed);
                println!("⬆️ Promoted '{}' from L2 to L1 with TTL {:?} (via get)", key, promotion_ttl);
            }

            // cleanup_guard will auto-remove entry on drop
            return Ok(Some(value));
        }
        
        // Both L1 and L2 miss
        self.misses.fetch_add(1, Ordering::Relaxed);
        
        // cleanup_guard will auto-remove entry on drop
        drop(cleanup_guard);
        
        Ok(None)
    }
    
    /// Get value from cache with fallback computation (enhanced backward compatibility)
    /// 
    /// This is a convenience method that combines `get()` with optional computation.
    /// If the value is not found in cache, it will execute the compute function
    /// and cache the result automatically.
    /// 
    /// # Arguments
    /// * `key` - Cache key
    /// * `compute_fn` - Optional function to compute value if not in cache
    /// * `strategy` - Cache strategy for storing computed value (default: ShortTerm)
    /// 
    /// # Returns
    /// * `Ok(Some(value))` - Value found in cache or computed successfully
    /// * `Ok(None)` - Value not in cache and no compute function provided
    /// * `Err(error)` - Cache operation or computation failed
    /// 
    /// # Example
    /// ```ignore
    /// // Simple cache get (existing behavior)
    /// let cached_data = cache_manager.get_with_fallback("my_key", None, None).await?;
    ///
    /// // Get with computation fallback (new enhanced behavior)
    /// let api_data = cache_manager.get_with_fallback(
    ///     "api_response",
    ///     Some(|| async { fetch_data_from_api().await }),
    ///     Some(CacheStrategy::RealTime)
    /// ).await?;
    /// ```
    
    /// Set value with specific cache strategy (all tiers)
    ///
    /// Supports both legacy 2-tier mode and new multi-tier mode (v0.5.0+).
    /// In multi-tier mode, stores to ALL tiers with their respective TTL scaling.
    pub async fn set_with_strategy(&self, key: &str, value: serde_json::Value, strategy: CacheStrategy) -> Result<()> {
        let ttl = strategy.to_duration();

        // NEW: Multi-tier mode (v0.5.0+)
        if let Some(tiers) = &self.tiers {
            // Store in ALL tiers with their respective TTL scaling
            let mut success_count = 0;
            let mut last_error = None;

            for tier in tiers {
                match tier.set_with_ttl(key, value.clone(), ttl).await {
                    Ok(_) => {
                        success_count += 1;
                    }
                    Err(e) => {
                        eprintln!("⚠️ L{} cache set failed for key '{}': {}", tier.tier_level, key, e);
                        last_error = Some(e);
                    }
                }
            }

            if success_count > 0 {
                println!("💾 [Multi-Tier] Cached '{}' in {}/{} tiers (base TTL: {:?})",
                         key, success_count, tiers.len(), ttl);
                return Ok(());
            } else {
                return Err(last_error.unwrap_or_else(|| anyhow::anyhow!("All tiers failed for key '{}'", key)));
            }
        }

        // LEGACY: 2-tier mode (L1 + L2)
        // Store in both L1 and L2
        let l1_result = self.l1_cache.set_with_ttl(key, value.clone(), ttl).await;
        let l2_result = self.l2_cache.set_with_ttl(key, value, ttl).await;

        // Return success if at least one cache succeeded
        match (l1_result, l2_result) {
            (Ok(_), Ok(_)) => {
                // Both succeeded
                println!("💾 [L1+L2] Cached '{}' with TTL {:?}", key, ttl);
                Ok(())
            }
            (Ok(_), Err(_)) => {
                // L1 succeeded, L2 failed
                eprintln!("⚠️ L2 cache set failed for key '{}', continuing with L1", key);
                println!("💾 [L1] Cached '{}' with TTL {:?}", key, ttl);
                Ok(())
            }
            (Err(_), Ok(_)) => {
                // L1 failed, L2 succeeded
                eprintln!("⚠️ L1 cache set failed for key '{}', continuing with L2", key);
                println!("💾 [L2] Cached '{}' with TTL {:?}", key, ttl);
                Ok(())
            }
            (Err(e1), Err(_e2)) => {
                // Both failed
                Err(anyhow::anyhow!("Both L1 and L2 cache set failed for key '{}': {}", key, e1))
            }
        }
    }
    
    /// Get or compute value with Cache Stampede protection across L1+L2+Compute
    /// 
    /// This method provides comprehensive Cache Stampede protection:
    /// 1. Check L1 cache first (uses Moka's built-in coalescing)
    /// 2. Check L2 cache with mutex-based coalescing
    /// 3. Compute fresh data with protection against concurrent computations
    /// 
    /// # Arguments
    /// * `key` - Cache key
    /// * `strategy` - Cache strategy for TTL and storage behavior
    /// * `compute_fn` - Async function to compute the value if not in any cache
    /// 
    /// # Example
    /// ```ignore
    /// let api_data = cache_manager.get_or_compute_with(
    ///     "api_response",
    ///     CacheStrategy::RealTime,
    ///     || async {
    ///         fetch_data_from_api().await
    ///     }
    /// ).await?;
    /// ```
    #[allow(dead_code)]
    pub async fn get_or_compute_with<F, Fut>(
        &self,
        key: &str,
        strategy: CacheStrategy,
        compute_fn: F,
    ) -> Result<serde_json::Value>
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = Result<serde_json::Value>> + Send,
    {
        self.total_requests.fetch_add(1, Ordering::Relaxed);
        
        // 1. Try L1 cache first (with built-in Moka coalescing for hot data)
        if let Some(value) = self.l1_cache.get(key).await {
            self.l1_hits.fetch_add(1, Ordering::Relaxed);
            return Ok(value);
        }
        
        // 2. L1 miss - try L2 with Cache Stampede protection
        let key_owned = key.to_string();
        let lock_guard = self.in_flight_requests
            .entry(key_owned.clone())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone();
        
        let _guard = lock_guard.lock().await;
        
        // RAII cleanup guard - ensures entry is removed even on early return or panic
        let _cleanup_guard = CleanupGuard {
            map: &self.in_flight_requests,
            key: key_owned,
        };
        
        // 3. Double-check L1 cache after acquiring lock
        // (Another request might have populated it while we were waiting)
        if let Some(value) = self.l1_cache.get(key).await {
            self.l1_hits.fetch_add(1, Ordering::Relaxed);
            // _cleanup_guard will auto-remove entry on drop
            return Ok(value);
        }

        // 4. Check remaining tiers (L2, L3, L4...) with Stampede protection
        if let Some(tiers) = &self.tiers {
            // Check tiers starting from index 1 (skip L1 since already checked)
            for tier in tiers.iter().skip(1) {
                if let Some((value, ttl)) = tier.get_with_ttl(key).await {
                    tier.record_hit();

                    // Promote to L1 (first tier)
                    let promotion_ttl = ttl.unwrap_or_else(|| strategy.to_duration());
                    if let Err(e) = tiers[0].set_with_ttl(key, value.clone(), promotion_ttl).await {
                        eprintln!("⚠️ Failed to promote '{}' from L{} to L1: {}", key, tier.tier_level, e);
                    } else {
                        self.promotions.fetch_add(1, Ordering::Relaxed);
                        println!("⬆️ Promoted '{}' from L{} to L1 with TTL {:?} (Stampede protected)",
                                key, tier.tier_level, promotion_ttl);
                    }

                    // _cleanup_guard will auto-remove entry on drop
                    return Ok(value);
                }
            }
        } else {
            // LEGACY: Check L2 cache with TTL
            if let Some((value, redis_ttl)) = self.l2_cache.get_with_ttl(key).await {
                self.l2_hits.fetch_add(1, Ordering::Relaxed);

                // Promote to L1 using Redis TTL (or strategy TTL as fallback)
                let promotion_ttl = redis_ttl.unwrap_or_else(|| strategy.to_duration());

                if let Err(e) = self.l1_cache.set_with_ttl(key, value.clone(), promotion_ttl).await {
                    eprintln!("⚠️ Failed to promote key '{}' to L1: {}", key, e);
                } else {
                    self.promotions.fetch_add(1, Ordering::Relaxed);
                    println!("⬆️ Promoted '{}' from L2 to L1 with TTL {:?}", key, promotion_ttl);
                }

                // _cleanup_guard will auto-remove entry on drop
                return Ok(value);
            }
        }

        // 5. Cache miss across all tiers - compute fresh data
        println!("💻 Computing fresh data for key: '{}' (Cache Stampede protected)", key);
        let fresh_data = compute_fn().await?;
        
        // 6. Store in both caches
        if let Err(e) = self.set_with_strategy(key, fresh_data.clone(), strategy).await {
            eprintln!("⚠️ Failed to cache computed data for key '{}': {}", key, e);
        }
        
        // 7. _cleanup_guard will auto-remove entry on drop

        Ok(fresh_data)
    }

    /// Get or compute typed value with Cache Stampede protection (Type-Safe Version)
    ///
    /// This method provides the same functionality as `get_or_compute_with()` but with
    /// **type-safe** automatic serialization/deserialization. Perfect for database queries,
    /// API calls, or any computation that returns structured data.
    ///
    /// # Type Safety
    ///
    /// - Returns your actual type `T` instead of `serde_json::Value`
    /// - Compiler enforces Serialize + DeserializeOwned bounds
    /// - No manual JSON conversion needed
    ///
    /// # Cache Flow
    ///
    /// 1. Check L1 cache → deserialize if found
    /// 2. Check L2 cache → deserialize + promote to L1 if found
    /// 3. Execute compute_fn → serialize → store in L1+L2
    /// 4. Full stampede protection (only ONE request computes)
    ///
    /// # Arguments
    ///
    /// * `key` - Cache key
    /// * `strategy` - Cache strategy for TTL
    /// * `compute_fn` - Async function returning `Result<T>`
    ///
    /// # Example - Database Query
    ///
    /// ```no_run
    /// # use multi_tier_cache::{CacheManager, CacheStrategy, L1Cache, L2Cache};
    /// # use std::sync::Arc;
    /// # use serde::{Serialize, Deserialize};
    /// # async fn example() -> anyhow::Result<()> {
    /// # let l1 = Arc::new(L1Cache::new().await?);
    /// # let l2 = Arc::new(L2Cache::new().await?);
    /// # let cache_manager = CacheManager::new(l1, l2);
    ///
    /// #[derive(Serialize, Deserialize)]
    /// struct User {
    ///     id: i64,
    ///     name: String,
    /// }
    ///
    /// // Type-safe database caching (example - requires sqlx)
    /// // let user: User = cache_manager.get_or_compute_typed(
    /// //     "user:123",
    /// //     CacheStrategy::MediumTerm,
    /// //     || async {
    /// //         sqlx::query_as::<_, User>("SELECT * FROM users WHERE id = $1")
    /// //             .bind(123)
    /// //             .fetch_one(&pool)
    /// //             .await
    /// //     }
    /// // ).await?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Example - API Call
    ///
    /// ```no_run
    /// # use multi_tier_cache::{CacheManager, CacheStrategy, L1Cache, L2Cache};
    /// # use std::sync::Arc;
    /// # use serde::{Serialize, Deserialize};
    /// # async fn example() -> anyhow::Result<()> {
    /// # let l1 = Arc::new(L1Cache::new().await?);
    /// # let l2 = Arc::new(L2Cache::new().await?);
    /// # let cache_manager = CacheManager::new(l1, l2);
    /// #[derive(Serialize, Deserialize)]
    /// struct ApiResponse {
    ///     data: String,
    ///     timestamp: i64,
    /// }
    ///
    /// // API call caching (example - requires reqwest)
    /// // let response: ApiResponse = cache_manager.get_or_compute_typed(
    /// //     "api:endpoint",
    /// //     CacheStrategy::RealTime,
    /// //     || async {
    /// //         reqwest::get("https://api.example.com/data")
    /// //             .await?
    /// //             .json::<ApiResponse>()
    /// //             .await
    /// //     }
    /// // ).await?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Performance
    ///
    /// - L1 hit: <1ms + deserialization (~10-50μs for small structs)
    /// - L2 hit: 2-5ms + deserialization + L1 promotion
    /// - Compute: Your function time + serialization + L1+L2 storage
    /// - Stampede protection: 99.6% latency reduction under high concurrency
    ///
    /// # Errors
    ///
    /// Returns error if:
    /// - Compute function fails
    /// - Serialization fails (invalid type for JSON)
    /// - Deserialization fails (cache data doesn't match type T)
    /// - Cache operations fail (Redis connection issues)
    pub async fn get_or_compute_typed<T, F, Fut>(
        &self,
        key: &str,
        strategy: CacheStrategy,
        compute_fn: F,
    ) -> Result<T>
    where
        T: serde::Serialize + serde::de::DeserializeOwned + Send + 'static,
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = Result<T>> + Send,
    {
        self.total_requests.fetch_add(1, Ordering::Relaxed);

        // 1. Try L1 cache first (with built-in Moka coalescing for hot data)
        if let Some(cached_json) = self.l1_cache.get(key).await {
            self.l1_hits.fetch_add(1, Ordering::Relaxed);

            // Attempt to deserialize from JSON to type T
            match serde_json::from_value::<T>(cached_json) {
                Ok(typed_value) => {
                    println!("✅ [L1 HIT] Deserialized '{}' to type {}", key, std::any::type_name::<T>());
                    return Ok(typed_value);
                }
                Err(e) => {
                    // Deserialization failed - cache data may be stale or corrupt
                    eprintln!("⚠️ L1 cache deserialization failed for key '{}': {}. Will recompute.", key, e);
                    // Fall through to recompute
                }
            }
        }

        // 2. L1 miss - try L2 with Cache Stampede protection
        let key_owned = key.to_string();
        let lock_guard = self.in_flight_requests
            .entry(key_owned.clone())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone();

        let _guard = lock_guard.lock().await;

        // RAII cleanup guard - ensures entry is removed even on early return or panic
        let _cleanup_guard = CleanupGuard {
            map: &self.in_flight_requests,
            key: key_owned,
        };

        // 3. Double-check L1 cache after acquiring lock
        // (Another request might have populated it while we were waiting)
        if let Some(cached_json) = self.l1_cache.get(key).await {
            self.l1_hits.fetch_add(1, Ordering::Relaxed);
            if let Ok(typed_value) = serde_json::from_value::<T>(cached_json) {
                println!("✅ [L1 HIT] Deserialized '{}' after lock acquisition", key);
                return Ok(typed_value);
            }
        }

        // 4. Check remaining tiers (L2, L3, L4...) with Stampede protection
        if let Some(tiers) = &self.tiers {
            // Check tiers starting from index 1 (skip L1 since already checked)
            for tier in tiers.iter().skip(1) {
                if let Some((cached_json, ttl)) = tier.get_with_ttl(key).await {
                    tier.record_hit();

                    // Attempt to deserialize
                    match serde_json::from_value::<T>(cached_json.clone()) {
                        Ok(typed_value) => {
                            println!("✅ [L{} HIT] Deserialized '{}' to type {}",
                                    tier.tier_level, key, std::any::type_name::<T>());

                            // Promote to L1 (first tier)
                            let promotion_ttl = ttl.unwrap_or_else(|| strategy.to_duration());
                            if let Err(e) = tiers[0].set_with_ttl(key, cached_json, promotion_ttl).await {
                                eprintln!("⚠️ Failed to promote '{}' from L{} to L1: {}",
                                         key, tier.tier_level, e);
                            } else {
                                self.promotions.fetch_add(1, Ordering::Relaxed);
                                println!("⬆️ Promoted '{}' from L{} to L1 with TTL {:?} (Stampede protected)",
                                        key, tier.tier_level, promotion_ttl);
                            }

                            return Ok(typed_value);
                        }
                        Err(e) => {
                            eprintln!("⚠️ L{} cache deserialization failed for key '{}': {}. Trying next tier.",
                                     tier.tier_level, key, e);
                            // Continue to next tier
                        }
                    }
                }
            }
        } else {
            // LEGACY: Check L2 cache with TTL
            if let Some((cached_json, redis_ttl)) = self.l2_cache.get_with_ttl(key).await {
                self.l2_hits.fetch_add(1, Ordering::Relaxed);

                // Attempt to deserialize
                match serde_json::from_value::<T>(cached_json.clone()) {
                    Ok(typed_value) => {
                        println!("✅ [L2 HIT] Deserialized '{}' from Redis", key);

                        // Promote to L1 using Redis TTL (or strategy TTL as fallback)
                        let promotion_ttl = redis_ttl.unwrap_or_else(|| strategy.to_duration());

                        if let Err(e) = self.l1_cache.set_with_ttl(key, cached_json, promotion_ttl).await {
                            eprintln!("⚠️ Failed to promote key '{}' to L1: {}", key, e);
                        } else {
                            self.promotions.fetch_add(1, Ordering::Relaxed);
                            println!("⬆️ Promoted '{}' from L2 to L1 with TTL {:?}", key, promotion_ttl);
                        }

                        return Ok(typed_value);
                    }
                    Err(e) => {
                        eprintln!("⚠️ L2 cache deserialization failed for key '{}': {}. Will recompute.", key, e);
                        // Fall through to recompute
                    }
                }
            }
        }

        // 5. Cache miss across all tiers (or deserialization failed) - compute fresh data
        println!("💻 Computing fresh typed data for key: '{}' (Cache Stampede protected)", key);
        let typed_value = compute_fn().await?;

        // 6. Serialize to JSON for storage
        let json_value = serde_json::to_value(&typed_value)
            .map_err(|e| anyhow::anyhow!("Failed to serialize type {} for caching: {}", std::any::type_name::<T>(), e))?;

        // 7. Store in both L1 and L2 caches
        if let Err(e) = self.set_with_strategy(key, json_value, strategy).await {
            eprintln!("⚠️ Failed to cache computed typed data for key '{}': {}", key, e);
        } else {
            println!("💾 Cached typed value for '{}' (type: {})", key, std::any::type_name::<T>());
        }

        // 8. _cleanup_guard will auto-remove entry on drop

        Ok(typed_value)
    }

    /// Get comprehensive cache statistics
    ///
    /// In multi-tier mode, aggregates statistics from all tiers.
    /// In legacy mode, returns L1 and L2 stats.
    #[allow(dead_code)]
    pub fn get_stats(&self) -> CacheManagerStats {
        let total_reqs = self.total_requests.load(Ordering::Relaxed);
        let l1_hits = self.l1_hits.load(Ordering::Relaxed);
        let l2_hits = self.l2_hits.load(Ordering::Relaxed);
        let misses = self.misses.load(Ordering::Relaxed);

        CacheManagerStats {
            total_requests: total_reqs,
            l1_hits,
            l2_hits,
            total_hits: l1_hits + l2_hits,
            misses,
            hit_rate: if total_reqs > 0 {
                ((l1_hits + l2_hits) as f64 / total_reqs as f64) * 100.0
            } else { 0.0 },
            l1_hit_rate: if total_reqs > 0 {
                (l1_hits as f64 / total_reqs as f64) * 100.0
            } else { 0.0 },
            promotions: self.promotions.load(Ordering::Relaxed),
            in_flight_requests: self.in_flight_requests.len(),
        }
    }

    /// Get per-tier statistics (v0.5.0+)
    ///
    /// Returns statistics for each tier if multi-tier mode is enabled.
    /// Returns None if using legacy 2-tier mode.
    ///
    /// # Example
    /// ```rust,ignore
    /// if let Some(tier_stats) = cache_manager.get_tier_stats() {
    ///     for stats in tier_stats {
    ///         println!("L{}: {} hits ({})",
    ///                  stats.tier_level,
    ///                  stats.hit_count(),
    ///                  stats.backend_name);
    ///     }
    /// }
    /// ```
    pub fn get_tier_stats(&self) -> Option<Vec<TierStats>> {
        self.tiers.as_ref().map(|tiers| {
            tiers.iter().map(|tier| tier.stats.clone()).collect()
        })
    }
    
    // ===== Redis Streams Methods =====
    
    /// Publish data to Redis Stream
    ///
    /// # Arguments
    /// * `stream_key` - Name of the stream (e.g., "events_stream")
    /// * `fields` - Field-value pairs to publish
    /// * `maxlen` - Optional max length for stream trimming
    ///
    /// # Returns
    /// The entry ID generated by Redis
    ///
    /// # Errors
    /// Returns error if streaming backend is not configured
    pub async fn publish_to_stream(
        &self,
        stream_key: &str,
        fields: Vec<(String, String)>,
        maxlen: Option<usize>
    ) -> Result<String> {
        match &self.streaming_backend {
            Some(backend) => backend.stream_add(stream_key, fields, maxlen).await,
            None => Err(anyhow::anyhow!("Streaming backend not configured"))
        }
    }
    
    /// Read latest entries from Redis Stream
    ///
    /// # Arguments
    /// * `stream_key` - Name of the stream
    /// * `count` - Number of latest entries to retrieve
    ///
    /// # Returns
    /// Vector of (entry_id, fields) tuples (newest first)
    ///
    /// # Errors
    /// Returns error if streaming backend is not configured
    pub async fn read_stream_latest(
        &self,
        stream_key: &str,
        count: usize
    ) -> Result<Vec<(String, Vec<(String, String)>)>> {
        match &self.streaming_backend {
            Some(backend) => backend.stream_read_latest(stream_key, count).await,
            None => Err(anyhow::anyhow!("Streaming backend not configured"))
        }
    }
    
    /// Read from Redis Stream with optional blocking
    ///
    /// # Arguments
    /// * `stream_key` - Name of the stream
    /// * `last_id` - Last ID seen ("0" for start, "$" for new only)
    /// * `count` - Max entries to retrieve
    /// * `block_ms` - Optional blocking timeout in ms
    ///
    /// # Returns
    /// Vector of (entry_id, fields) tuples
    ///
    /// # Errors
    /// Returns error if streaming backend is not configured
    pub async fn read_stream(
        &self,
        stream_key: &str,
        last_id: &str,
        count: usize,
        block_ms: Option<usize>
    ) -> Result<Vec<(String, Vec<(String, String)>)>> {
        match &self.streaming_backend {
            Some(backend) => backend.stream_read(stream_key, last_id, count, block_ms).await,
            None => Err(anyhow::anyhow!("Streaming backend not configured"))
        }
    }

    // ===== Cache Invalidation Methods =====

    /// Invalidate a cache key across all instances
    ///
    /// This removes the key from all cache tiers and broadcasts
    /// the invalidation to all other cache instances via Redis Pub/Sub.
    ///
    /// Supports both legacy 2-tier mode and new multi-tier mode (v0.5.0+).
    ///
    /// # Arguments
    /// * `key` - Cache key to invalidate
    ///
    /// # Example
    /// ```rust,ignore
    /// // Invalidate user cache after profile update
    /// cache_manager.invalidate("user:123").await?;
    /// ```
    pub async fn invalidate(&self, key: &str) -> Result<()> {
        // NEW: Multi-tier mode (v0.5.0+)
        if let Some(tiers) = &self.tiers {
            // Remove from ALL tiers
            for tier in tiers {
                if let Err(e) = tier.remove(key).await {
                    eprintln!("⚠️ Failed to remove '{}' from L{}: {}", key, tier.tier_level, e);
                }
            }
        } else {
            // LEGACY: 2-tier mode
            self.l1_cache.remove(key).await?;
            self.l2_cache.remove(key).await?;
        }

        // Broadcast to other instances
        if let Some(publisher) = &self.invalidation_publisher {
            let mut pub_lock = publisher.lock().await;
            let msg = InvalidationMessage::remove(key);
            pub_lock.publish(&msg).await?;
            self.invalidation_stats.messages_sent.fetch_add(1, Ordering::Relaxed);
        }

        println!("🗑️  Invalidated '{}' across all instances", key);
        Ok(())
    }

    /// Update cache value across all instances
    ///
    /// This updates the key in all cache tiers and broadcasts
    /// the update to all other cache instances, avoiding cache misses.
    ///
    /// Supports both legacy 2-tier mode and new multi-tier mode (v0.5.0+).
    ///
    /// # Arguments
    /// * `key` - Cache key to update
    /// * `value` - New value
    /// * `ttl` - Optional TTL (uses default if None)
    ///
    /// # Example
    /// ```rust,ignore
    /// // Update user cache with new data
    /// let user_data = serde_json::json!({"id": 123, "name": "Alice"});
    /// cache_manager.update_cache("user:123", user_data, Some(Duration::from_secs(3600))).await?;
    /// ```
    pub async fn update_cache(
        &self,
        key: &str,
        value: serde_json::Value,
        ttl: Option<Duration>,
    ) -> Result<()> {
        let ttl = ttl.unwrap_or_else(|| CacheStrategy::Default.to_duration());

        // NEW: Multi-tier mode (v0.5.0+)
        if let Some(tiers) = &self.tiers {
            // Update ALL tiers with their respective TTL scaling
            for tier in tiers {
                if let Err(e) = tier.set_with_ttl(key, value.clone(), ttl).await {
                    eprintln!("⚠️ Failed to update '{}' in L{}: {}", key, tier.tier_level, e);
                }
            }
        } else {
            // LEGACY: 2-tier mode
            self.l1_cache.set_with_ttl(key, value.clone(), ttl).await?;
            self.l2_cache.set_with_ttl(key, value.clone(), ttl).await?;
        }

        // Broadcast update to other instances
        if let Some(publisher) = &self.invalidation_publisher {
            let mut pub_lock = publisher.lock().await;
            let msg = InvalidationMessage::update(key, value, Some(ttl));
            pub_lock.publish(&msg).await?;
            self.invalidation_stats.messages_sent.fetch_add(1, Ordering::Relaxed);
        }

        println!("🔄 Updated '{}' across all instances", key);
        Ok(())
    }

    /// Invalidate all keys matching a pattern
    ///
    /// This scans L2 cache for keys matching the pattern, removes them from all tiers,
    /// and broadcasts the invalidation. L1 caches will be cleared via broadcast.
    ///
    /// Supports both legacy 2-tier mode and new multi-tier mode (v0.5.0+).
    ///
    /// **Note**: Pattern scanning requires a concrete L2Cache instance with `scan_keys()`.
    /// In multi-tier mode, this scans from L2 but removes from all tiers.
    ///
    /// # Arguments
    /// * `pattern` - Glob-style pattern (e.g., "user:*", "product:123:*")
    ///
    /// # Example
    /// ```rust,ignore
    /// // Invalidate all user caches
    /// cache_manager.invalidate_pattern("user:*").await?;
    ///
    /// // Invalidate specific user's related caches
    /// cache_manager.invalidate_pattern("user:123:*").await?;
    /// ```
    pub async fn invalidate_pattern(&self, pattern: &str) -> Result<()> {
        // Scan L2 for matching keys
        // (Note: Pattern scanning requires concrete L2Cache with scan_keys support)
        let keys = if let Some(l2) = &self.l2_cache_concrete {
            l2.scan_keys(pattern).await?
        } else {
            return Err(anyhow::anyhow!("Pattern invalidation requires concrete L2Cache instance"));
        };

        if keys.is_empty() {
            println!("🔍 No keys found matching pattern '{}'", pattern);
            return Ok(());
        }

        // NEW: Multi-tier mode (v0.5.0+)
        if let Some(tiers) = &self.tiers {
            // Remove from ALL tiers
            for key in &keys {
                for tier in tiers {
                    if let Err(e) = tier.remove(key).await {
                        eprintln!("⚠️ Failed to remove '{}' from L{}: {}", key, tier.tier_level, e);
                    }
                }
            }
        } else {
            // LEGACY: 2-tier mode - Remove from L2 in bulk
            if let Some(l2) = &self.l2_cache_concrete {
                l2.remove_bulk(&keys).await?;
            }
        }

        // Broadcast pattern invalidation
        if let Some(publisher) = &self.invalidation_publisher {
            let mut pub_lock = publisher.lock().await;
            let msg = InvalidationMessage::remove_bulk(keys.clone());
            pub_lock.publish(&msg).await?;
            self.invalidation_stats.messages_sent.fetch_add(1, Ordering::Relaxed);
        }

        println!("🔍 Invalidated {} keys matching pattern '{}'", keys.len(), pattern);
        Ok(())
    }

    /// Set value with automatic broadcast to all instances
    ///
    /// This is a write-through operation that updates the cache and
    /// broadcasts the update to all other instances automatically.
    ///
    /// # Arguments
    /// * `key` - Cache key
    /// * `value` - Value to cache
    /// * `strategy` - Cache strategy (determines TTL)
    ///
    /// # Example
    /// ```rust,ignore
    /// // Update and broadcast in one call
    /// let data = serde_json::json!({"status": "active"});
    /// cache_manager.set_with_broadcast("user:123", data, CacheStrategy::MediumTerm).await?;
    /// ```
    pub async fn set_with_broadcast(
        &self,
        key: &str,
        value: serde_json::Value,
        strategy: CacheStrategy,
    ) -> Result<()> {
        let ttl = strategy.to_duration();

        // Set in local caches
        self.set_with_strategy(key, value.clone(), strategy).await?;

        // Broadcast update if invalidation is enabled
        if let Some(publisher) = &self.invalidation_publisher {
            let mut pub_lock = publisher.lock().await;
            let msg = InvalidationMessage::update(key, value, Some(ttl));
            pub_lock.publish(&msg).await?;
            self.invalidation_stats.messages_sent.fetch_add(1, Ordering::Relaxed);
        }

        Ok(())
    }

    /// Get invalidation statistics
    ///
    /// Returns statistics about invalidation operations if invalidation is enabled.
    pub fn get_invalidation_stats(&self) -> Option<InvalidationStats> {
        if self.invalidation_subscriber.is_some() {
            Some(self.invalidation_stats.snapshot())
        } else {
            None
        }
    }
}

/// Cache Manager statistics
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct CacheManagerStats {
    pub total_requests: u64,
    pub l1_hits: u64,
    pub l2_hits: u64,
    pub total_hits: u64,
    pub misses: u64,
    pub hit_rate: f64,
    pub l1_hit_rate: f64,
    pub promotions: usize,
    pub in_flight_requests: usize,
}
