//! Cache Manager - Unified Cache Operations
//!
//! Manages operations across L1 (Moka) and L2 (Redis) caches with intelligent fallback.

use crate::error::CacheResult;
use dashmap::DashMap;
use rand::Rng;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;
#[cfg(feature = "redis")]
use tokio::sync::Mutex;
use tokio::sync::broadcast;
use tracing::{debug, error, info, warn};

#[cfg(feature = "moka")]
use crate::L1Cache;
#[cfg(feature = "redis")]
use crate::L2Cache;
#[cfg(feature = "redis")]
use crate::invalidation::{
    AtomicInvalidationStats, InvalidationConfig, InvalidationMessage, InvalidationPublisher,
    InvalidationSubscriber,
};
use crate::serialization::{CacheSerializer, JsonSerializer};
use crate::traits::{CacheBackend, L2CacheBackend, StreamingBackend};
use bytes::Bytes;
use futures_util::future::BoxFuture;

///// Type alias for the in-flight requests map
/// Stores a broadcast sender for each active key computation
type InFlightMap = DashMap<String, broadcast::Sender<CacheResult<Bytes>>>;

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
    #[must_use]
    pub fn to_duration(&self) -> Duration {
        match self {
            Self::RealTime => Duration::from_secs(10),
            Self::ShortTerm | Self::Default => Duration::from_secs(300), // 5 minutes
            Self::MediumTerm => Duration::from_secs(3600),               // 1 hour
            Self::LongTerm => Duration::from_secs(10800),                // 3 hours
            Self::Custom(duration) => *duration,
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
#[derive(Clone)]
pub struct CacheTier {
    /// The backend for this tier
    pub backend: Arc<dyn L2CacheBackend>,
    /// Tier level (1 for fastest, increases for slower/cheaper tiers)
    pub tier_level: usize,
    /// Whether to promote keys FROM lower tiers TO this tier
    pub promotion_enabled: bool,
    /// Promotion frequency (N) - promote with 1/N probability
    pub promotion_frequency: usize,
    /// TTL multiplier for this tier (e.g., L2 might store for 2x L1 TTL)
    pub ttl_scale: f64,
    /// Statistics for this tier
    pub stats: TierStats,
}

impl CacheTier {
    /// Create a new cache tier
    pub fn new(
        backend: Arc<dyn L2CacheBackend>,
        tier_level: usize,
        promotion_enabled: bool,
        promotion_frequency: usize,
        ttl_scale: f64,
    ) -> Self {
        let backend_name = backend.name().to_string();
        Self {
            backend,
            tier_level,
            promotion_enabled,
            promotion_frequency,
            ttl_scale,
            stats: TierStats::new(tier_level, backend_name),
        }
    }

    /// Get value with TTL from this tier
    async fn get_with_ttl(&self, key: &str) -> Option<(Bytes, Option<Duration>)> {
        self.backend.get_with_ttl(key).await
    }

    /// Set value with TTL in this tier
    async fn set_with_ttl(&self, key: &str, value: Bytes, ttl: Duration) -> CacheResult<()> {
        let scaled_ttl = Duration::from_secs_f64(ttl.as_secs_f64() * self.ttl_scale);
        self.backend.set_with_ttl(key, value, scaled_ttl).await
    }

    /// Remove value from this tier
    async fn remove(&self, key: &str) -> CacheResult<()> {
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
    /// Promotion frequency (N) - promote with 1/N probability (default 10)
    pub promotion_frequency: usize,
    /// TTL scale factor (1.0 = same as base TTL)
    pub ttl_scale: f64,
}

impl TierConfig {
    /// Create new tier configuration
    #[must_use]
    pub fn new(tier_level: usize) -> Self {
        Self {
            tier_level,
            promotion_enabled: true,
            promotion_frequency: 10,
            ttl_scale: 1.0,
        }
    }

    /// Configure as L1 (hot tier)
    #[must_use]
    pub fn as_l1() -> Self {
        Self {
            tier_level: 1,
            promotion_enabled: false, // L1 is already top tier
            promotion_frequency: 1,   // Doesn't matter but use 1
            ttl_scale: 1.0,
        }
    }

    /// Configure as L2 (warm tier)
    #[must_use]
    pub fn as_l2() -> Self {
        Self {
            tier_level: 2,
            promotion_enabled: true,
            promotion_frequency: 10,
            ttl_scale: 1.0,
        }
    }

    /// Configure as L3 (cold tier) with longer TTL
    #[must_use]
    pub fn as_l3() -> Self {
        Self {
            tier_level: 3,
            promotion_enabled: true,
            promotion_frequency: 10,
            ttl_scale: 2.0, // Keep data 2x longer
        }
    }

    /// Configure as L4 (archive tier) with much longer TTL
    #[must_use]
    pub fn as_l4() -> Self {
        Self {
            tier_level: 4,
            promotion_enabled: true,
            promotion_frequency: 10,
            ttl_scale: 8.0, // Keep data 8x longer
        }
    }

    /// Set promotion enabled
    #[must_use]
    pub fn with_promotion(mut self, enabled: bool) -> Self {
        self.promotion_enabled = enabled;
        self
    }

    /// Set promotion frequency (N)
    #[must_use]
    pub fn with_promotion_frequency(mut self, n: usize) -> Self {
        self.promotion_frequency = n;
        self
    }

    /// Set TTL scale factor
    #[must_use]
    pub fn with_ttl_scale(mut self, scale: f64) -> Self {
        self.ttl_scale = scale;
        self
    }

    /// Set tier level
    #[must_use]
    pub fn with_level(mut self, level: usize) -> Self {
        self.tier_level = level;
        self
    }
}

pub struct CacheManager {
    /// Ordered list of cache tiers (L1, L2, L3, ...)
    tiers: Vec<CacheTier>,

    /// Optional streaming backend
    streaming_backend: Option<Arc<dyn StreamingBackend>>,
    /// Statistics
    total_requests: AtomicU64,
    l1_hits: AtomicU64,
    l2_hits: AtomicU64,
    misses: AtomicU64,
    /// In-flight requests map (Broadcaster integration will replace this in Step 4)
    in_flight_requests: Arc<InFlightMap>,
    /// Pluggable serializer
    serializer: Arc<CacheSerializer>,
    /// Invalidation publisher
    #[cfg(feature = "redis")]
    invalidation_publisher: Option<Arc<Mutex<InvalidationPublisher>>>,
    /// Invalidation subscriber
    #[cfg(feature = "redis")]
    invalidation_subscriber: Option<Arc<InvalidationSubscriber>>,
    /// Invalidation statistics
    #[cfg(feature = "redis")]
    invalidation_stats: Arc<AtomicInvalidationStats>,
    /// Number of promotions performed
    promotions: AtomicUsize,
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
    /// # Errors
    ///
    /// Returns `Ok` if successful. Currently no error conditions, but kept for future compatibility.
    pub fn new_with_backends(
        l1_cache: Arc<dyn CacheBackend>,
        l2_cache: Arc<dyn L2CacheBackend>,
        streaming_backend: Option<Arc<dyn StreamingBackend>>,
    ) -> CacheResult<Self> {
        debug!("Initializing Cache Manager with L1+L2 backends...");

        let tiers = vec![
            CacheTier::new(Arc::new(ProxyL1ToL2(l1_cache)), 1, false, 1, 1.0),
            CacheTier::new(l2_cache, 2, true, 10, 1.0),
        ];

        Ok(Self {
            tiers,
            streaming_backend,
            total_requests: AtomicU64::new(0),
            l1_hits: AtomicU64::new(0),
            l2_hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            promotions: AtomicUsize::new(0),
            in_flight_requests: Arc::new(DashMap::new()),
            serializer: Arc::new(CacheSerializer::Json(JsonSerializer)),
            #[cfg(feature = "redis")]
            invalidation_publisher: None,
            #[cfg(feature = "redis")]
            invalidation_subscriber: None,
            #[cfg(feature = "redis")]
            #[cfg(feature = "redis")]
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
    /// # Errors
    ///
    /// Returns an error if Redis connection fails.
    #[cfg(all(feature = "moka", feature = "redis"))]
    pub async fn new(l1_cache: Arc<L1Cache>, l2_cache: Arc<L2Cache>) -> CacheResult<Self> {
        debug!("Initializing Cache Manager...");

        // Convert concrete types to trait objects
        let l1_backend: Arc<dyn CacheBackend> = l1_cache.clone();
        let l2_backend: Arc<dyn L2CacheBackend> = l2_cache.clone();

        // Create RedisStreams backend for streaming functionality
        let streaming_backend: Option<Arc<dyn StreamingBackend>> = {
            let redis_url =
                std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());
            let redis_streams = crate::redis_streams::RedisStreams::new(&redis_url).await?;
            Some(Arc::new(redis_streams))
        };

        Self::new_with_backends(l1_backend, l2_backend, streaming_backend)
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
    /// # Errors
    ///
    /// Returns an error if Redis connection fails or invalidation setup fails.
    #[cfg(all(feature = "moka", feature = "redis"))]
    pub async fn new_with_invalidation(
        l1_cache: Arc<L1Cache>,
        l2_cache: Arc<L2Cache>,
        redis_url: &str,
        config: InvalidationConfig,
    ) -> CacheResult<Self> {
        debug!("Initializing Cache Manager with Invalidation...");
        debug!("  Pub/Sub channel: {}", config.channel);

        // Convert concrete types to trait objects
        let l1_backend: Arc<dyn CacheBackend> = l1_cache.clone();
        let l2_backend: Arc<dyn L2CacheBackend> = l2_cache.clone();

        // Create RedisStreams backend for streaming functionality
        let streaming_backend: Option<Arc<dyn StreamingBackend>> = {
            let redis_streams = crate::redis_streams::RedisStreams::new(redis_url).await?;
            Some(Arc::new(redis_streams))
        };

        // Create publisher
        let (invalidation_publisher, invalidation_subscriber) = {
            let client = redis::Client::open(redis_url)?;
            let conn_manager = redis::aio::ConnectionManager::new(client).await?;
            let publisher = InvalidationPublisher::new(conn_manager, config.clone());

            // Create subscriber
            let subscriber = InvalidationSubscriber::new(redis_url, config.clone())?;
            (
                Some(Arc::new(Mutex::new(publisher))),
                Some(Arc::new(subscriber)),
            )
        };

        let invalidation_stats = Arc::new(AtomicInvalidationStats::default());

        let tiers = vec![
            CacheTier::new(Arc::new(ProxyL1ToL2(l1_backend)), 1, false, 1, 1.0),
            CacheTier::new(l2_backend, 2, true, 10, 1.0),
        ];

        let manager = Self {
            tiers,
            streaming_backend,
            total_requests: AtomicU64::new(0),
            l1_hits: AtomicU64::new(0),
            l2_hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            promotions: AtomicUsize::new(0),
            in_flight_requests: Arc::new(DashMap::new()),
            serializer: Arc::new(CacheSerializer::Json(JsonSerializer)),
            invalidation_publisher,
            invalidation_subscriber,
            invalidation_stats,
        };

        // Start subscriber with handler
        manager.start_invalidation_subscriber();

        info!("Cache Manager initialized with invalidation support");

        Ok(manager)
    }

    /// Create new cache manager with multi-tier architecture (v0.5.0+)
    ///
    /// This constructor enables dynamic multi-tier caching with 3, 4, or more tiers.
    /// Tiers are checked in order (lower `tier_level` = faster/hotter).
    ///
    /// # Arguments
    ///
    /// * `tiers` - Vector of configured cache tiers (must be sorted by `tier_level` ascending)
    /// * `streaming_backend` - Optional streaming backend
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use multi_tier_cache::{CacheManager, CacheTier, TierConfig, L1Cache, L2Cache};
    /// use std::sync::Arc;
    ///
    /// // L1 + L2 + L3 setup
    /// let l1 = Arc::new(L1Cache::new()?);
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
    /// # Errors
    ///
    /// Returns an error if tiers are not sorted by level or if no tiers are provided.
    pub fn new_with_tiers(
        tiers: Vec<CacheTier>,
        streaming_backend: Option<Arc<dyn StreamingBackend>>,
    ) -> CacheResult<Self> {
        debug!("Initializing Multi-Tier Cache Manager...");
        debug!("  Tier count: {}", tiers.len());
        for tier in &tiers {
            debug!(
                "  L{}: {} (promotion={}, ttl_scale={})",
                tier.tier_level, tier.stats.backend_name, tier.promotion_enabled, tier.ttl_scale
            );
        }

        // Validate tiers are sorted by level
        for i in 1..tiers.len() {
            if let (Some(current), Some(prev)) = (tiers.get(i), tiers.get(i - 1))
                && current.tier_level <= prev.tier_level
            {
                return Err(crate::error::CacheError::ConfigError(format!(
                    "Tiers must be sorted by tier_level ascending (found L{} after L{})",
                    current.tier_level, prev.tier_level
                )));
            }
        }

        Ok(Self {
            tiers,
            streaming_backend,
            total_requests: AtomicU64::new(0),
            l1_hits: AtomicU64::new(0),
            l2_hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            promotions: AtomicUsize::new(0),
            in_flight_requests: Arc::new(DashMap::new()),
            serializer: Arc::new(CacheSerializer::Json(JsonSerializer)),
            #[cfg(feature = "redis")]
            invalidation_publisher: None,
            #[cfg(feature = "redis")]
            invalidation_subscriber: None,
            #[cfg(feature = "redis")]
            #[cfg(feature = "redis")]
            invalidation_stats: Arc::new(AtomicInvalidationStats::default()),
        })
    }

    /// Set a custom serializer for the cache manager
    pub fn set_serializer(&mut self, serializer: CacheSerializer) {
        debug!(name = %serializer.name(), "Switching cache serializer");
        self.serializer = Arc::new(serializer);
    }

    /// Start the invalidation subscriber background task
    #[cfg(feature = "redis")]
    fn start_invalidation_subscriber(&self) {
        #[cfg(feature = "redis")]
        if let Some(subscriber) = &self.invalidation_subscriber {
            let tiers = self.tiers.clone();

            subscriber.start(move |msg: crate::invalidation::InvalidationMessage| {
                let tiers = tiers.clone();
                async move {
                    for tier in &tiers {
                        match &msg {
                            InvalidationMessage::Remove { key } => {
                                tier.backend.remove(key).await.ok();
                            }
                            InvalidationMessage::Update {
                                key,
                                value,
                                ttl_secs,
                            } => {
                                if let Some(secs) = ttl_secs {
                                    tier.backend
                                        .set_with_ttl(
                                            key,
                                            value.clone(),
                                            Duration::from_secs(*secs),
                                        )
                                        .await
                                        .ok();
                                } else {
                                    tier.backend.set(key, value.clone()).await.ok();
                                }
                            }
                            InvalidationMessage::RemovePattern { pattern } => {
                                if let Err(e) = tier.backend.remove_pattern(pattern).await {
                                    warn!(
                                        "Failed to remove pattern '{}' from L{}: {}",
                                        pattern, tier.tier_level, e
                                    );
                                }
                            }
                            InvalidationMessage::RemoveBulk { keys } => {
                                for key in keys {
                                    if let Err(e) = tier.backend.remove(key).await {
                                        warn!(
                                            "Failed to remove '{}' from L{}: {}",
                                            key, tier.tier_level, e
                                        );
                                    }
                                }
                            }
                        }
                    }
                    Ok(())
                }
            });

            info!("Invalidation subscriber started across all tiers");
        }
    }

    /// Get value from cache using multi-tier architecture (v0.5.0+)
    ///
    /// This method iterates through all configured tiers and automatically promotes
    /// to upper tiers on cache hit.
    async fn get_multi_tier(&self, key: &str) -> CacheResult<Option<Bytes>> {
        // Try each tier sequentially (sorted by tier_level)
        for (tier_index, tier) in self.tiers.iter().enumerate() {
            if let Some((value, ttl)) = tier.get_with_ttl(key).await {
                // Cache hit!
                tier.record_hit();

                // Promote to all upper tiers (if promotion enabled)
                if tier.promotion_enabled && tier_index > 0 {
                    // Probabilistic Promotion Check
                    let should_promote = if tier.promotion_frequency <= 1 {
                        true
                    } else {
                        rand::thread_rng().gen_ratio(
                            1,
                            u32::try_from(tier.promotion_frequency).unwrap_or(u32::MAX),
                        )
                    };

                    if should_promote {
                        let promotion_ttl =
                            ttl.unwrap_or_else(|| CacheStrategy::Default.to_duration());

                        // Promote to all tiers above this one
                        for upper_tier in self.tiers.iter().take(tier_index).rev() {
                            if let Err(e) = upper_tier
                                .set_with_ttl(key, value.clone(), promotion_ttl)
                                .await
                            {
                                warn!(
                                    "Failed to promote '{}' from L{} to L{}: {}",
                                    key, tier.tier_level, upper_tier.tier_level, e
                                );
                            } else {
                                self.promotions.fetch_add(1, Ordering::Relaxed);
                                debug!(
                                    "Promoted '{}' from L{} to L{} (TTL: {:?})",
                                    key, tier.tier_level, upper_tier.tier_level, promotion_ttl
                                );
                            }
                        }
                    } else {
                        debug!(
                            "Probabilistic skip promotion for '{}' from L{} (N={})",
                            key, tier.tier_level, tier.promotion_frequency
                        );
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
    /// # Errors
    ///
    /// Returns an error if cache operation fails.
    ///
    /// # Panics
    ///
    /// Panics if tiers are not initialized in multi-tier mode (should not happen if constructed correctly).
    pub async fn get(&self, key: &str) -> CacheResult<Option<Bytes>> {
        self.total_requests.fetch_add(1, Ordering::Relaxed);

        // Fast path for L1 (first tier) - no locking needed
        if let Some(tier1) = self.tiers.first()
            && let Some((value, _ttl)) = tier1.get_with_ttl(key).await
        {
            tier1.record_hit();
            // Update legacy stats for backward compatibility
            self.l1_hits.fetch_add(1, Ordering::Relaxed);
            return Ok(Some(value));
        }

        // L1 miss - use stampede protection for lower tiers
        let key_owned = key.to_string();
        let lock_guard = self
            .in_flight_requests
            .entry(key_owned.clone())
            .or_insert_with(|| broadcast::Sender::new(1))
            .clone();

        let mut rx = lock_guard.subscribe();

        // If there are other receivers, someone else is computing, wait for it
        if lock_guard.receiver_count() > 1 {
            match rx.recv().await {
                Ok(Ok(value)) => return Ok(Some(value)),
                Ok(Err(e)) => {
                    return Err(crate::error::CacheError::BackendError(format!(
                        "Computation failed in another thread: {e}"
                    )));
                }
                Err(_) => {} // Fall through to re-compute if sender dropped or channel empty
            }
        }

        // Double-check L1 after acquiring lock (or if we are the first to compute)
        if let Some(tier1) = self.tiers.first()
            && let Some((value, _ttl)) = tier1.get_with_ttl(key).await
        {
            tier1.record_hit();
            self.l1_hits.fetch_add(1, Ordering::Relaxed);
            let _ = lock_guard.send(Ok(value.clone())); // Notify any waiting subscribers
            return Ok(Some(value));
        }

        // Check remaining tiers with promotion
        let result = self.get_multi_tier(key).await?;

        if let Some(val) = result.clone() {
            // Hit in L2+ tier - update legacy stats
            if self.tiers.len() >= 2 {
                self.l2_hits.fetch_add(1, Ordering::Relaxed);
            }

            // Notify any waiting subscribers
            let _ = lock_guard.send(Ok(val.clone()));

            // Remove the in-flight entry after computation/retrieval
            self.in_flight_requests.remove(key);

            Ok(Some(val))
        } else {
            self.misses.fetch_add(1, Ordering::Relaxed);

            // Remove the in-flight entry after computation/retrieval
            self.in_flight_requests.remove(key);

            Ok(None)
        }
    }

    /// Get a value from cache and deserialize it (Type-Safe Version)
    ///
    /// # Errors
    ///
    /// Returns a `SerializationError` if deserialization fails, or a `BackendError` if the cache retrieval fails.
    pub async fn get_typed<T>(&self, key: &str) -> CacheResult<Option<T>>
    where
        T: serde::de::DeserializeOwned,
    {
        if let Some(bytes) = self.get(key).await? {
            return Ok(Some(self.serializer.deserialize::<T>(&bytes)?));
        }
        Ok(None)
    }

    /// Set value with specific cache strategy (all tiers)
    ///
    /// Supports both legacy 2-tier mode and new multi-tier mode (v0.5.0+).
    /// In multi-tier mode, stores to ALL tiers with their respective TTL scaling.
    /// # Errors
    ///
    /// Returns an error if cache set operation fails.
    pub async fn set_with_strategy(
        &self,
        key: &str,
        value: Bytes,
        strategy: CacheStrategy,
    ) -> CacheResult<()> {
        let ttl = strategy.to_duration();

        let mut success_count = 0;
        let mut last_error = None;

        for tier in &self.tiers {
            match tier.set_with_ttl(key, value.clone(), ttl).await {
                Ok(()) => {
                    success_count += 1;
                }
                Err(e) => {
                    error!(
                        "L{} cache set failed for key '{}': {}",
                        tier.tier_level, key, e
                    );
                    last_error = Some(e);
                }
            }
        }

        if success_count > 0 {
            debug!(
                "[Cache] Stored '{}' in {}/{} tiers (base TTL: {:?})",
                key,
                success_count,
                self.tiers.len(),
                ttl
            );
            return Ok(());
        }

        Err(last_error.unwrap_or_else(|| {
            crate::error::CacheError::InternalError("All tiers failed".to_string())
        }))
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
    /// # Errors
    ///
    /// Returns an error if compute function fails or cache operations fail.
    pub async fn get_or_compute_with<F, Fut>(
        &self,
        key: &str,
        strategy: CacheStrategy,
        compute_fn: F,
    ) -> CacheResult<Bytes>
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = CacheResult<Bytes>> + Send,
    {
        self.total_requests.fetch_add(1, Ordering::Relaxed);

        // 1. Try tiers sequentially first
        for (idx, tier) in self.tiers.iter().enumerate() {
            if let Some((value, _ttl)) = tier.get_with_ttl(key).await {
                tier.record_hit();
                if tier.tier_level == 1 {
                    self.l1_hits.fetch_add(1, Ordering::Relaxed);
                } else if tier.tier_level == 2 {
                    self.l2_hits.fetch_add(1, Ordering::Relaxed);
                }

                // Promotion to L1 if hit was in a lower tier
                if idx > 0 && tier.promotion_enabled {
                    // Probabilistic Promotion Check
                    let should_promote = if tier.promotion_frequency <= 1 {
                        true
                    } else {
                        rand::thread_rng().gen_ratio(
                            1,
                            u32::try_from(tier.promotion_frequency).unwrap_or(u32::MAX),
                        )
                    };

                    if should_promote && let Some(l1_tier) = self.tiers.first() {
                        let _ = l1_tier
                            .set_with_ttl(key, value.clone(), strategy.to_duration())
                            .await;
                        self.promotions.fetch_add(1, Ordering::Relaxed);
                    }
                }
                return Ok(value);
            }
        }

        // 2. Cache miss across all tiers - use stampede protection
        let (tx, mut rx): (
            tokio::sync::broadcast::Sender<CacheResult<Bytes>>,
            tokio::sync::broadcast::Receiver<CacheResult<Bytes>>,
        ) = match self.in_flight_requests.entry(key.to_string()) {
            dashmap::mapref::entry::Entry::Occupied(entry) => {
                let tx = entry.get().clone();
                (tx.clone(), tx.subscribe())
            }
            dashmap::mapref::entry::Entry::Vacant(entry) => {
                let (tx, _) = tokio::sync::broadcast::channel(1);
                entry.insert(tx.clone());
                (tx.clone(), tx.subscribe())
            }
        };

        if tx.receiver_count() > 1 {
            // Someone else is computing, wait for it
            match rx.recv().await {
                Ok(Ok(value)) => return Ok(value),
                Ok(Err(e)) => {
                    return Err(crate::error::CacheError::BackendError(format!(
                        "Computation failed in another thread: {e}"
                    )));
                }
                Err(_) => {} // Fall through to re-compute
            }
        }

        // 3. Re-check cache after receiving/creating broadcaster (double-check pattern)
        for tier in &self.tiers {
            if let Some((value, _)) = tier.get_with_ttl(key).await {
                let _ = tx.send(Ok(value.clone()));
                return Ok(value);
            }
        }

        // 4. Miss - compute fresh data
        debug!(
            "Computing fresh data for key: '{}' (Stampede protected)",
            key
        );

        let result = compute_fn().await;

        // Remove from in_flight BEFORE broadcasting
        self.in_flight_requests.remove(key);

        match &result {
            Ok(value) => {
                let _ = self.set_with_strategy(key, value.clone(), strategy).await;
                let _ = tx.send(Ok(value.clone()));
            }
            Err(e) => {
                let _ = tx.send(Err(e.clone()));
            }
        }

        result
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
    /// - Compiler enforces Serialize + `DeserializeOwned` bounds
    /// - No manual JSON conversion needed
    ///
    /// # Cache Flow
    ///
    /// 1. Check L1 cache → deserialize if found
    /// 2. Check L2 cache → deserialize + promote to L1 if found
    /// 3. Execute `compute_fn` → serialize → store in L1+L2
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
    /// # use multi_tier_cache::{CacheManager, CacheStrategy, L1Cache, L2Cache, MokaCacheConfig};
    /// # use std::sync::Arc;
    /// # use serde::{Serialize, Deserialize};
    /// # async fn example() -> anyhow::Result<()> {
    /// # let l1 = Arc::new(L1Cache::new(MokaCacheConfig::default())?);
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
    /// # use multi_tier_cache::{CacheManager, CacheStrategy, L1Cache, L2Cache, MokaCacheConfig};
    /// # use std::sync::Arc;
    /// # use serde::{Serialize, Deserialize};
    /// # async fn example() -> anyhow::Result<()> {
    /// # let l1 = Arc::new(L1Cache::new(MokaCacheConfig::default())?);
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
    #[allow(clippy::too_many_lines)]
    pub async fn get_or_compute_typed<T, F, Fut>(
        &self,
        key: &str,
        strategy: CacheStrategy,
        compute_fn: F,
    ) -> CacheResult<T>
    where
        T: serde::Serialize + serde::de::DeserializeOwned + Send + Sync + 'static,
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = CacheResult<T>> + Send,
    {
        // 1. Try to get typed from cache first
        if let Some(value) = self.get_typed::<T>(key).await? {
            return Ok(value);
        }

        // 2. Use get_or_compute_with to handle stampede protection
        let serializer = self.serializer.clone();
        let bytes_result = self
            .get_or_compute_with(key, strategy, || async move {
                let val = compute_fn().await?;
                serializer.serialize(&val)
            })
            .await?;

        // 3. Deserialize result
        self.serializer.deserialize::<T>(&bytes_result)
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
                #[allow(clippy::cast_precision_loss)]
                {
                    ((l1_hits + l2_hits) as f64 / total_reqs as f64) * 100.0
                }
            } else {
                0.0
            },
            l1_hit_rate: if total_reqs > 0 {
                #[allow(clippy::cast_precision_loss)]
                {
                    (l1_hits as f64 / total_reqs as f64) * 100.0
                }
            } else {
                0.0
            },
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
    pub fn get_tier_stats(&self) -> Vec<TierStats> {
        self.tiers.iter().map(|tier| tier.stats.clone()).collect()
    }
}

/// Proxy wrapper to allow using `CacheBackend` where `DynL2CacheBackend` is expected
/// (Internal helper for `new_with_backends` to wrap L1 `CacheBackend` into `DynL2CacheBackend`)
struct ProxyL1ToL2(Arc<dyn CacheBackend>);

impl CacheBackend for ProxyL1ToL2 {
    fn get<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Option<Bytes>> {
        self.0.get(key)
    }

    fn set_with_ttl<'a>(
        &'a self,
        key: &'a str,
        value: Bytes,
        ttl: Duration,
    ) -> BoxFuture<'a, CacheResult<()>> {
        self.0.set_with_ttl(key, value, ttl)
    }

    fn remove<'a>(&'a self, key: &'a str) -> BoxFuture<'a, CacheResult<()>> {
        self.0.remove(key)
    }

    fn remove_pattern<'a>(&'a self, pattern: &'a str) -> BoxFuture<'a, CacheResult<()>> {
        self.0.remove_pattern(pattern)
    }

    fn health_check(&self) -> BoxFuture<'_, bool> {
        self.0.health_check()
    }

    fn name(&self) -> &'static str {
        self.0.name()
    }
}

impl L2CacheBackend for ProxyL1ToL2 {
    fn get_with_ttl<'a>(
        &'a self,
        key: &'a str,
    ) -> BoxFuture<'a, Option<(Bytes, Option<Duration>)>> {
        Box::pin(async move { self.0.get(key).await.map(|v| (v, None)) })
    }
}

impl CacheManager {
    // ===== Redis Streams Methods =====

    /// Publish data to Redis Stream
    ///
    /// # Arguments
    /// * `stream_key` - Name of the stream (e.g., "`events_stream`")
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
        maxlen: Option<usize>,
    ) -> CacheResult<String> {
        match &self.streaming_backend {
            Some(backend) => backend.stream_add(stream_key, fields, maxlen).await,
            None => Err(crate::error::CacheError::ConfigError(
                "Streaming backend not configured".to_string(),
            )),
        }
    }

    /// Read latest entries from Redis Stream
    ///
    /// # Arguments
    /// * `stream_key` - Name of the stream
    /// * `count` - Number of latest entries to retrieve
    ///
    /// # Returns
    /// Vector of (`entry_id`, fields) tuples (newest first)
    ///
    /// # Errors
    /// Returns error if streaming backend is not configured
    pub async fn read_stream_latest(
        &self,
        stream_key: &str,
        count: usize,
    ) -> CacheResult<Vec<(String, Vec<(String, String)>)>> {
        match &self.streaming_backend {
            Some(backend) => backend
                .stream_read_latest(stream_key, count)
                .await,
            None => Err(crate::error::CacheError::ConfigError(
                "Streaming backend not configured".to_string(),
            )),
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
    /// Vector of (`entry_id`, fields) tuples
    ///
    /// # Errors
    /// Returns error if streaming backend is not configured
    pub async fn read_stream(
        &self,
        stream_key: &str,
        last_id: &str,
        count: usize,
        block_ms: Option<usize>,
    ) -> CacheResult<Vec<(String, Vec<(String, String)>)>> {
        match &self.streaming_backend {
            Some(backend) => {
                backend
                    .stream_read(stream_key, last_id, count, block_ms)
                    .await
            }
            None => Err(crate::error::CacheError::ConfigError(
                "Streaming backend not configured".to_string(),
            )),
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
    /// ```rust,no_run
    /// # use multi_tier_cache::CacheManager;
    /// # async fn example(cache_manager: &CacheManager) -> anyhow::Result<()> {
    /// // Invalidate user cache after profile update
    /// cache_manager.invalidate("user:123").await?;
    /// # Ok(())
    /// # }
    /// ```
    /// # Errors
    ///
    /// Returns an error if invalidation fails.
    pub async fn invalidate(&self, key: &str) -> CacheResult<()> {
        // Remove from ALL tiers
        for tier in &self.tiers {
            if let Err(e) = tier.remove(key).await {
                warn!(
                    "Failed to remove '{}' from L{}: {}",
                    key, tier.tier_level, e
                );
            }
        }

        // Broadcast to other instances
        #[cfg(feature = "redis")]
        {
            if let Some(publisher) = &self.invalidation_publisher {
                let mut pub_lock: tokio::sync::MutexGuard<
                    '_,
                    crate::invalidation::InvalidationPublisher,
                > = publisher.lock().await;
                let msg = InvalidationMessage::remove(key);
                pub_lock.publish(&msg).await?;
                self.invalidation_stats
                    .messages_sent
                    .fetch_add(1, Ordering::Relaxed);
            }
        }

        debug!("Invalidated '{}' across all instances", key);
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
    /// ```rust,no_run
    /// # use multi_tier_cache::CacheManager;
    /// # use std::time::Duration;
    /// # use bytes::Bytes;
    /// # async fn example(cache_manager: &CacheManager) -> anyhow::Result<()> {
    /// // Update user cache with new data
    /// let user_data = Bytes::from("alice");
    /// cache_manager.update_cache("user:123", user_data, Some(Duration::from_secs(3600))).await?;
    /// # Ok(())
    /// # }
    /// ```
    /// # Errors
    ///
    /// Returns an error if cache update fails.
    pub async fn update_cache(
        &self,
        key: &str,
        value: Bytes,
        ttl: Option<Duration>,
    ) -> CacheResult<()> {
        let ttl = ttl.unwrap_or_else(|| CacheStrategy::Default.to_duration());

        // Update ALL tiers
        for tier in &self.tiers {
            if let Err(e) = tier.set_with_ttl(key, value.clone(), ttl).await {
                warn!("Failed to update '{}' in L{}: {}", key, tier.tier_level, e);
            }
        }

        // Broadcast update to other instances
        #[cfg(feature = "redis")]
        if let Some(publisher) = &self.invalidation_publisher {
            let mut pub_lock = publisher.lock().await;
            let msg = InvalidationMessage::update(key, value, Some(ttl));
            pub_lock.publish(&msg).await?;
            self.invalidation_stats
                .messages_sent
                .fetch_add(1, Ordering::Relaxed);
        }

        debug!("Updated '{}' across all instances", key);
        Ok(())
    }

    /// Invalidate all keys matching a pattern
    ///
    /// This scans L2 cache for keys matching the pattern, removes them from all tiers,
    /// and broadcasts the invalidation. L1 caches will be cleared via broadcast.
    ///
    /// Supports both legacy 2-tier mode and new multi-tier mode (v0.5.0+).
    ///
    /// **Note**: Pattern scanning requires a concrete `L2Cache` instance with `scan_keys()`.
    /// In multi-tier mode, this scans from L2 but removes from all tiers.
    ///
    /// # Arguments
    /// * `pattern` - Glob-style pattern (e.g., "user:*", "product:123:*")
    ///
    /// # Example
    /// ```rust,no_run
    /// # use multi_tier_cache::CacheManager;
    /// # async fn example(cache_manager: &CacheManager) -> anyhow::Result<()> {
    /// // Invalidate all user caches
    /// cache_manager.invalidate_pattern("user:*").await?;
    ///
    /// // Invalidate specific user's related caches
    /// cache_manager.invalidate_pattern("user:123:*").await?;
    /// # Ok(())
    /// # }
    /// ```
    /// # Errors
    ///
    /// Returns an error if invalidation fails.
    pub async fn invalidate_pattern(&self, pattern: &str) -> CacheResult<()> {
        debug!(pattern = %pattern, "Invalidating pattern across all tiers");

        // 1. Invalidate in all configured tiers
        for tier in &self.tiers {
            debug!(tier = %tier.tier_level, "Invalidating pattern in tier");
            tier.backend.remove_pattern(pattern).await?;
        }

        // 2. Broadcast invalidation if publisher is configured
        #[cfg(feature = "redis")]
        {
            if let Some(publisher) = &self.invalidation_publisher {
                let msg = InvalidationMessage::remove_pattern(pattern);
                publisher.lock().await.publish(&msg).await?;
                debug!(pattern = %pattern, "Broadcasted pattern invalidation");
            }
        }

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
    /// ```rust,no_run
    /// # use multi_tier_cache::{CacheManager, CacheStrategy};
    /// # use bytes::Bytes;
    /// # async fn example(cache_manager: &CacheManager) -> anyhow::Result<()> {
    /// // Update and broadcast in one call
    /// let data = Bytes::from("active");
    /// cache_manager.set_with_broadcast("user:123", data, CacheStrategy::MediumTerm).await?;
    /// # Ok(())
    /// # }
    /// ```
    /// # Errors
    ///
    /// Returns an error if cache set or broadcast fails.
    pub async fn set_with_broadcast(
        &self,
        key: &str,
        value: Bytes,
        strategy: CacheStrategy,
    ) -> CacheResult<()> {
        #[cfg(feature = "redis")] let ttl = strategy.to_duration();

        // Set in local caches
        self.set_with_strategy(key, value.clone(), strategy).await?;

        // Broadcast update if invalidation is enabled
        #[cfg(feature = "redis")]
        if let Some(publisher) = &self.invalidation_publisher {
            let mut pub_lock = publisher.lock().await;
            let msg = InvalidationMessage::update(key, value, Some(ttl));
            pub_lock.publish(&msg).await?;
            self.invalidation_stats
                .messages_sent
                .fetch_add(1, Ordering::Relaxed);
        }

        Ok(())
    }

    /// Get invalidation statistics
    ///
    /// Returns statistics about invalidation operations if invalidation is enabled.
    #[cfg(feature = "redis")]
    pub fn invalidation_stats(&self) -> Option<crate::invalidation::InvalidationStats> {
        #[cfg(feature = "redis")]
        {
            Some(self.invalidation_stats.snapshot())
        }
        #[cfg(not(feature = "redis"))]
        {
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
