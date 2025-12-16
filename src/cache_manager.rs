//! Cache Manager - Unified Cache Operations
//!
//! Manages operations across L1 (Moka) and L2 (Redis) caches with intelligent fallback.

use anyhow::Result;
use dashmap::DashMap;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json;
use std::future::Future;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

use tracing::{debug, error, info, warn};

use super::invalidation::{
    AtomicInvalidationStats, InvalidationConfig, InvalidationMessage, InvalidationPublisher,
    InvalidationStats, InvalidationSubscriber,
};
use crate::backends::{L1Cache, L2Cache};
use crate::codecs::JsonCodec; // Allow default to be JsonCodec
use crate::traits::{CacheBackend, CacheCodec, L2CacheBackend, StreamingBackend};

/// Type alias for the in-flight requests map
type InFlightMap = DashMap<String, Arc<Mutex<()>>>;

/// RAII cleanup guard for in-flight request tracking
/// Ensures that entries are removed from `DashMap` even on early return or panic
struct CleanupGuard<'a> {
    map: &'a InFlightMap,
    key: String,
}

impl Drop for CleanupGuard<'_> {
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
    async fn get_with_ttl(&self, key: &str) -> Option<(Vec<u8>, Option<Duration>)> {
        self.backend.get_with_ttl(key).await
    }

    /// Set value with TTL in this tier
    async fn set_with_ttl(&self, key: &str, value: &[u8], ttl: Duration) -> Result<()> {
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
    #[must_use]
    pub fn new(tier_level: usize) -> Self {
        Self {
            tier_level,
            promotion_enabled: true,
            ttl_scale: 1.0,
        }
    }

    /// Configure as L1 (hot tier)
    #[must_use]
    pub fn as_l1() -> Self {
        Self {
            tier_level: 1,
            promotion_enabled: false, // L1 is already top tier
            ttl_scale: 1.0,
        }
    }

    /// Configure as L2 (warm tier)
    #[must_use]
    pub fn as_l2() -> Self {
        Self {
            tier_level: 2,
            promotion_enabled: true,
            ttl_scale: 1.0,
        }
    }

    /// Configure as L3 (cold tier) with longer TTL
    #[must_use]
    pub fn as_l3() -> Self {
        Self {
            tier_level: 3,
            promotion_enabled: true,
            ttl_scale: 2.0, // Keep data 2x longer
        }
    }

    /// Configure as L4 (archive tier) with much longer TTL
    #[must_use]
    pub fn as_l4() -> Self {
        Self {
            tier_level: 4,
            promotion_enabled: true,
            ttl_scale: 8.0, // Keep data 8x longer
        }
    }

    /// Set promotion enabled
    #[must_use]
    pub fn with_promotion(mut self, enabled: bool) -> Self {
        self.promotion_enabled = enabled;
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

/// Proxy wrapper to convert `L2CacheBackend` to `CacheBackend`
/// (Rust doesn't support automatic trait upcasting for trait objects)
struct ProxyCacheBackend {
    backend: Arc<dyn L2CacheBackend>,
}

#[async_trait::async_trait]
impl CacheBackend for ProxyCacheBackend {
    async fn get(&self, key: &str) -> Option<Vec<u8>> {
        self.backend.get(key).await
    }

    async fn set_with_ttl(&self, key: &str, value: &[u8], ttl: Duration) -> Result<()> {
        self.backend.set_with_ttl(key, value, ttl).await
    }

    async fn remove(&self, key: &str) -> Result<()> {
        self.backend.remove(key).await
    }

    async fn health_check(&self) -> bool {
        self.backend.health_check().await
    }

    fn name(&self) -> &'static str {
        self.backend.name()
    }
}

/// Inner struct containing the heavy cache manager logic and state.
/// This struct is wrapped by `CacheManager` to allow cheap cloning via Arc.
pub struct CacheManagerInner<C: CacheCodec = JsonCodec> {
    /// Dynamic multi-tier cache architecture (v0.5.0+)
    /// If Some, this takes precedence over `l1_cache/l2_cache` fields
    tiers: Option<Vec<CacheTier>>,

    // ===== Legacy fields (v0.1.0 - v0.4.x) =====
    // Maintained for backward compatibility
    /// L1 Cache (trait object for pluggable backends)
    l1_cache: Arc<dyn CacheBackend>,
    /// L2 Cache (trait object for pluggable backends)
    l2_cache: Arc<dyn L2CacheBackend>,
    /// L2 Cache concrete instance (for invalidation `scan_keys`)
    l2_cache_concrete: Option<Arc<L2Cache>>,

    /// Optional streaming backend (defaults to L2 if it implements `StreamingBackend`)
    streaming_backend: Option<Arc<dyn StreamingBackend>>,

    /// Serialization codec
    codec: Arc<C>,

    /// Statistics (`AtomicU64` is already thread-safe, no Arc needed)
    total_requests: AtomicU64,
    l1_hits: AtomicU64,
    l2_hits: AtomicU64,
    misses: AtomicU64,
    promotions: AtomicUsize,
    /// In-flight requests to prevent Cache Stampede on L2/compute operations
    in_flight_requests: InFlightMap,
    /// Invalidation publisher (for broadcasting invalidation messages)
    invalidation_publisher: Option<Mutex<InvalidationPublisher>>,
    /// Invalidation subscriber (for receiving invalidation messages)
    invalidation_subscriber: Option<InvalidationSubscriber>,
    /// Invalidation statistics
    invalidation_stats: AtomicInvalidationStats,
}

/// Cache Manager - Unified operations across multiple cache tiers
/// Wrapper around `CacheManagerInner` to support cheap cloning.
#[derive(Clone)]
pub struct CacheManager<C: CacheCodec = JsonCodec> {
    inner: Arc<CacheManagerInner<C>>,
}

impl CacheManager<JsonCodec> {
    // Forward to generic constructor
    pub fn new_with_backends(
        l1_cache: Arc<dyn CacheBackend>,
        l2_cache: Arc<dyn L2CacheBackend>,
        streaming_backend: Option<Arc<dyn StreamingBackend>>,
    ) -> Result<Self> {
        Self::with_codec(l1_cache, l2_cache, streaming_backend, JsonCodec::default())
    }

    pub async fn new(l1_cache: Arc<L1Cache>, l2_cache: Arc<L2Cache>) -> Result<Self> {
        // Convert concrete types to trait objects
        let l1_backend: Arc<dyn CacheBackend> = l1_cache.clone();
        let l2_backend: Arc<dyn L2CacheBackend> = l2_cache.clone();

        // Create RedisStreams backend for streaming functionality
        let redis_url =
            std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());
        let redis_streams = crate::redis_streams::RedisStreams::new(&redis_url).await?;
        let streaming_backend: Arc<dyn StreamingBackend> = Arc::new(redis_streams);

        Self::new_with_backends(l1_backend, l2_backend, Some(streaming_backend))
    }

    pub async fn new_with_invalidation(
        l1_cache: Arc<L1Cache>,
        l2_cache: Arc<L2Cache>,
        redis_url: &str,
        config: InvalidationConfig,
    ) -> Result<Self> {
        debug!("Initializing Cache Manager with Invalidation...");
        debug!("  Pub/Sub channel: {}", config.channel);

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
        let invalidation_stats = AtomicInvalidationStats::default();

        let codec = Arc::new(JsonCodec::default());

        let inner = CacheManagerInner {
            tiers: None, // Legacy mode: use l1_cache/l2_cache fields
            l1_cache: l1_backend,
            l2_cache: l2_backend,
            l2_cache_concrete: Some(l2_cache),
            streaming_backend: Some(streaming_backend),
            codec,
            total_requests: AtomicU64::new(0),
            l1_hits: AtomicU64::new(0),
            l2_hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            promotions: AtomicUsize::new(0),
            in_flight_requests: DashMap::new(),
            invalidation_publisher: Some(Mutex::new(publisher)),
            invalidation_subscriber: Some(subscriber),
            invalidation_stats,
        };

        let manager = Self {
            inner: Arc::new(inner),
        };
        manager.start_invalidation_subscriber();
        info!("Cache Manager initialized with invalidation support");
        Ok(manager)
    }

    pub fn new_with_tiers(
        tiers: Vec<CacheTier>,
        streaming_backend: Option<Arc<dyn StreamingBackend>>,
    ) -> Result<Self> {
        Self::with_tiers_and_codec(tiers, streaming_backend, JsonCodec::default())
    }
}

impl<C: CacheCodec + 'static> CacheManager<C> {
    /// Create a new cache manager with specific codec
    pub fn with_codec(
        l1_cache: Arc<dyn CacheBackend>,
        l2_cache: Arc<dyn L2CacheBackend>,
        streaming_backend: Option<Arc<dyn StreamingBackend>>,
        codec: C,
    ) -> Result<Self> {
        debug!("Initializing Cache Manager with custom backends and codec...");
        debug!("  L1: {}", l1_cache.name());
        debug!("  L2: {}", l2_cache.name());
        debug!("  Codec: {}", codec.name());

        let inner = CacheManagerInner {
            tiers: None, // Legacy mode: use l1_cache/l2_cache fields
            l1_cache,
            l2_cache,
            l2_cache_concrete: None,
            streaming_backend,
            codec: Arc::new(codec),
            total_requests: AtomicU64::new(0),
            l1_hits: AtomicU64::new(0),
            l2_hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            promotions: AtomicUsize::new(0),
            in_flight_requests: DashMap::new(),
            invalidation_publisher: None,
            invalidation_subscriber: None,
            invalidation_stats: AtomicInvalidationStats::default(),
        };

        Ok(Self {
            inner: Arc::new(inner),
        })
    }

    pub fn with_tiers_and_codec(
        tiers: Vec<CacheTier>,
        streaming_backend: Option<Arc<dyn StreamingBackend>>,
        codec: C,
    ) -> Result<Self> {
        debug!("Initializing Multi-Tier Cache Manager with Codec...");
        debug!("  Tier count: {}", tiers.len());
        debug!("  Codec: {}", codec.name());
        for tier in &tiers {
            debug!(
                "  L{}: {} (promotion={}, ttl_scale={})",
                tier.tier_level, tier.stats.backend_name, tier.promotion_enabled, tier.ttl_scale
            );
        }

        // Validate tiers are sorted by level
        for i in 1..tiers.len() {
            if let (Some(current), Some(prev)) = (tiers.get(i), tiers.get(i - 1)) {
                if current.tier_level <= prev.tier_level {
                    anyhow::bail!(
                        "Tiers must be sorted by tier_level ascending (found L{} after L{})",
                        current.tier_level,
                        prev.tier_level
                    );
                }
            }
        }

        // For backward compatibility with legacy code, we need dummy l1/l2 caches
        // Use first tier as l1, second tier as l2 if available
        let (l1_cache, l2_cache) = if tiers.len() >= 2 {
            if let (Some(t0), Some(t1)) = (tiers.first(), tiers.get(1)) {
                (t0.backend.clone(), t1.backend.clone())
            } else {
                anyhow::bail!("Failed to access tiers 0 and 1");
            }
        } else if tiers.len() == 1 {
            // Only one tier - use it for both
            if let Some(t0) = tiers.first() {
                let tier = t0.backend.clone();
                (tier.clone(), tier)
            } else {
                anyhow::bail!("Failed to access tier 0");
            }
        } else {
            anyhow::bail!("At least one cache tier is required");
        };

        // Convert to CacheBackend trait for l1 (L2CacheBackend extends CacheBackend)
        let l1_backend: Arc<dyn CacheBackend> = Arc::new(ProxyCacheBackend {
            backend: l1_cache.clone(),
        });

        let inner = CacheManagerInner {
            tiers: Some(tiers),
            l1_cache: l1_backend,
            l2_cache,
            l2_cache_concrete: None,
            streaming_backend,
            codec: Arc::new(codec),
            total_requests: AtomicU64::new(0),
            l1_hits: AtomicU64::new(0),
            l2_hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            promotions: AtomicUsize::new(0),
            in_flight_requests: DashMap::new(),
            invalidation_publisher: None,
            invalidation_subscriber: None,
            invalidation_stats: AtomicInvalidationStats::default(),
        };

        Ok(Self {
            inner: Arc::new(inner),
        })
    }

    /// Start the invalidation subscriber background task
    fn start_invalidation_subscriber(&self) {
        if let Some(subscriber) = &self.inner.invalidation_subscriber {
            let l1_cache = Arc::clone(&self.inner.l1_cache);
            let l2_cache_concrete = self.inner.l2_cache_concrete.clone();
            let codec = Arc::clone(&self.inner.codec);

            subscriber.start(move |msg| {
                let l1 = Arc::clone(&l1_cache);
                let _l2 = l2_cache_concrete.clone();
                let codec = Arc::clone(&codec);

                async move {
                    match msg {
                        InvalidationMessage::Remove { key } => {
                            l1.remove(&key).await?;
                            debug!("Invalidation: Removed '{}' from L1", key);
                        }
                        InvalidationMessage::Update {
                            key,
                            value,
                            ttl_secs,
                        } => match codec.serialize(&value) {
                            Ok(bytes) => {
                                let ttl = ttl_secs
                                    .map_or_else(|| Duration::from_secs(300), Duration::from_secs);
                                l1.set_with_ttl(&key, &bytes, ttl).await?;
                                debug!("Invalidation: Updated '{}' in L1", key);
                            }
                            Err(e) => {
                                warn!(
                                    "Invalidation: Failed to serialize value for '{}': {}",
                                    key, e
                                );
                            }
                        },
                        InvalidationMessage::RemovePattern { pattern } => {
                            debug!(
                                "Invalidation: Pattern '{}' invalidated (L1 will expire naturally)",
                                pattern
                            );
                        }
                        InvalidationMessage::RemoveBulk { keys } => {
                            for key in keys {
                                if let Err(e) = l1.remove(&key).await {
                                    warn!("Failed to remove '{}' from L1: {}", key, e);
                                }
                            }
                            debug!("Invalidation: Bulk removed keys from L1");
                        }
                    }
                    Ok(())
                }
            });

            info!("Invalidation subscriber started");
        }
    }

    async fn get_multi_tier(&self, key: &str) -> Result<Option<Vec<u8>>> {
        let Some(tiers) = self.inner.tiers.as_ref() else {
            panic!("Tiers must be initialized in multi-tier mode")
        };

        for (tier_index, tier) in tiers.iter().enumerate() {
            if let Some((value, ttl)) = tier.get_with_ttl(key).await {
                tier.record_hit();

                if tier.promotion_enabled && tier_index > 0 {
                    let promotion_ttl = ttl.unwrap_or_else(|| CacheStrategy::Default.to_duration());

                    for upper_tier in tiers.iter().take(tier_index).rev() {
                        if let Err(e) = upper_tier.set_with_ttl(key, &value, promotion_ttl).await {
                            warn!(
                                "Failed to promote '{}' from L{} to L{}: {}",
                                key, tier.tier_level, upper_tier.tier_level, e
                            );
                        } else {
                            self.inner.promotions.fetch_add(1, Ordering::Relaxed);
                            debug!(
                                "Promoted '{}' from L{} to L{} (TTL: {:?})",
                                key, tier.tier_level, upper_tier.tier_level, promotion_ttl
                            );
                        }
                    }
                }

                return Ok(Some(value));
            }
        }

        Ok(None)
    }

    pub async fn get<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        self.inner.total_requests.fetch_add(1, Ordering::Relaxed);

        let deserialize = |bytes: Vec<u8>| -> Result<Option<T>> {
            match self.inner.codec.deserialize(&bytes) {
                Ok(v) => Ok(Some(v)),
                Err(e) => {
                    error!("Failed to deserialize cache value for key '{}': {}", key, e);
                    Err(e)
                }
            }
        };

        if self.inner.tiers.is_some() {
            if let Some(tier1) = self.inner.tiers.as_ref().unwrap().first() {
                if let Some((value, _ttl)) = tier1.get_with_ttl(key).await {
                    tier1.record_hit();
                    self.inner.l1_hits.fetch_add(1, Ordering::Relaxed);
                    return deserialize(value);
                }
            }

            let key_owned = key.to_string();
            let lock_guard = self
                .inner
                .in_flight_requests
                .entry(key_owned.clone())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone();

            let _guard = lock_guard.lock().await;
            let cleanup_guard = CleanupGuard {
                map: &self.inner.in_flight_requests,
                key: key_owned.clone(),
            };

            if let Some(tier1) = self.inner.tiers.as_ref().unwrap().first() {
                if let Some((value, _ttl)) = tier1.get_with_ttl(key).await {
                    tier1.record_hit();
                    self.inner.l1_hits.fetch_add(1, Ordering::Relaxed);
                    return deserialize(value);
                }
            }

            let result_bytes = self.get_multi_tier(key).await?;

            if result_bytes.is_some() {
                if self.inner.tiers.as_ref().unwrap().len() >= 2 {
                    self.inner.l2_hits.fetch_add(1, Ordering::Relaxed);
                }
                return deserialize(result_bytes.unwrap());
            } else {
                self.inner.misses.fetch_add(1, Ordering::Relaxed);
            }

            drop(cleanup_guard);
            return Ok(None);
        }

        if let Some(value) = self.inner.l1_cache.get(key).await {
            self.inner.l1_hits.fetch_add(1, Ordering::Relaxed);
            return deserialize(value);
        }

        let key_owned = key.to_string();
        let lock_guard = self
            .inner
            .in_flight_requests
            .entry(key_owned.clone())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone();

        let _guard = lock_guard.lock().await;

        let cleanup_guard = CleanupGuard {
            map: &self.inner.in_flight_requests,
            key: key_owned.clone(),
        };

        if let Some(value) = self.inner.l1_cache.get(key).await {
            self.inner.l1_hits.fetch_add(1, Ordering::Relaxed);
            return deserialize(value);
        }

        if let Some((value, ttl)) = self.inner.l2_cache.get_with_ttl(key).await {
            self.inner.l2_hits.fetch_add(1, Ordering::Relaxed);

            let promotion_ttl = ttl.unwrap_or_else(|| CacheStrategy::Default.to_duration());

            if self
                .inner
                .l1_cache
                .set_with_ttl(key, &value, promotion_ttl)
                .await
                .is_err()
            {
                warn!("Failed to promote key '{}' to L1 cache", key);
            } else {
                self.inner.promotions.fetch_add(1, Ordering::Relaxed);
                debug!(
                    "Promoted '{}' from L2 to L1 with TTL {:?} (via get)",
                    key, promotion_ttl
                );
            }

            return deserialize(value);
        }

        self.inner.misses.fetch_add(1, Ordering::Relaxed);
        drop(cleanup_guard);

        Ok(None)
    }

    pub async fn set_with_strategy<T: Serialize>(
        &self,
        key: &str,
        value: &T,
        strategy: CacheStrategy,
    ) -> Result<()> {
        let ttl = strategy.to_duration();
        let bytes = self.inner.codec.serialize(value)?;

        if let Some(tiers) = &self.inner.tiers {
            let mut success_count = 0;
            let mut last_error = None;

            for tier in tiers {
                match tier.set_with_ttl(key, &bytes, ttl).await {
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
                    "[Multi-Tier] Cached '{}' in {}/{} tiers (base TTL: {:?})",
                    key,
                    success_count,
                    tiers.len(),
                    ttl
                );
                return Ok(());
            }
            return Err(
                last_error.unwrap_or_else(|| anyhow::anyhow!("All tiers failed for key '{key}'"))
            );
        }

        let l1_result = self.inner.l1_cache.set_with_ttl(key, &bytes, ttl).await;
        let l2_result = self.inner.l2_cache.set_with_ttl(key, &bytes, ttl).await;

        match (l1_result, l2_result) {
            (Ok(()), Ok(())) => {
                debug!("[L1+L2] Cached '{}' with TTL {:?}", key, ttl);
                Ok(())
            }
            (Ok(()), Err(_)) => {
                warn!("L2 cache set failed for key '{}', continuing with L1", key);
                debug!("[L1] Cached '{}' with TTL {:?}", key, ttl);
                Ok(())
            }
            (Err(_), Ok(())) => {
                warn!("L1 cache set failed for key '{}', continuing with L2", key);
                debug!("[L2] Cached '{}' with TTL {:?}", key, ttl);
                Ok(())
            }
            (Err(e1), Err(_e2)) => Err(anyhow::anyhow!(
                "Both L1 and L2 cache set failed for key '{key}': {e1}"
            )),
        }
    }

    pub async fn get_or_compute<T, F, Fut>(
        &self,
        key: &str,
        strategy: CacheStrategy,
        compute_fn: F,
    ) -> Result<T>
    where
        T: Serialize + DeserializeOwned + Send + Sync + 'static,
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = Result<T>> + Send,
    {
        self.inner.total_requests.fetch_add(1, Ordering::Relaxed);

        let deserialize = |bytes: Vec<u8>| -> Result<Option<T>> {
            match self.inner.codec.deserialize(&bytes) {
                Ok(v) => Ok(Some(v)),
                Err(e) => {
                    error!("Failed to deserialize cache value for key '{}': {}", key, e);
                    Err(e)
                }
            }
        };

        if let Some(bytes) = self.inner.l1_cache.get(key).await {
            self.inner.l1_hits.fetch_add(1, Ordering::Relaxed);
            if let Ok(Some(val)) = deserialize(bytes) {
                return Ok(val);
            }
        }

        let key_owned = key.to_string();
        let lock_guard = self
            .inner
            .in_flight_requests
            .entry(key_owned.clone())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone();

        let _guard = lock_guard.lock().await;

        let _cleanup_guard = CleanupGuard {
            map: &self.inner.in_flight_requests,
            key: key_owned,
        };

        if let Some(bytes) = self.inner.l1_cache.get(key).await {
            self.inner.l1_hits.fetch_add(1, Ordering::Relaxed);
            if let Ok(Some(val)) = deserialize(bytes) {
                return Ok(val);
            }
        }

        let found_bytes = if self.inner.tiers.is_some() {
            self.get_multi_tier(key).await?
        } else {
            if let Some((bytes, ttl)) = self.inner.l2_cache.get_with_ttl(key).await {
                let promotion_ttl = ttl.unwrap_or_else(|| strategy.to_duration());
                let _ = self
                    .inner
                    .l1_cache
                    .set_with_ttl(key, &bytes, promotion_ttl)
                    .await;
                self.inner.l2_hits.fetch_add(1, Ordering::Relaxed);
                self.inner.promotions.fetch_add(1, Ordering::Relaxed);
                Some(bytes)
            } else {
                None
            }
        };

        if let Some(bytes) = found_bytes {
            if let Ok(Some(val)) = deserialize(bytes) {
                return Ok(val);
            }
        }

        debug!("Computing fresh data for key: '{}'", key);
        let fresh_data = compute_fn().await?;

        if let Err(e) = self.set_with_strategy(key, &fresh_data, strategy).await {
            warn!("Failed to cache computed data for key '{}': {}", key, e);
        }

        Ok(fresh_data)
    }

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
        self.get_or_compute(key, strategy, compute_fn).await
    }

    #[allow(dead_code)]
    pub fn get_stats(&self) -> CacheManagerStats {
        let total_reqs = self.inner.total_requests.load(Ordering::Relaxed);
        let l1_hits = self.inner.l1_hits.load(Ordering::Relaxed);
        let l2_hits = self.inner.l2_hits.load(Ordering::Relaxed);
        let misses = self.inner.misses.load(Ordering::Relaxed);

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
            promotions: self.inner.promotions.load(Ordering::Relaxed),
            in_flight_requests: self.inner.in_flight_requests.len(),
        }
    }

    pub fn get_tier_stats(&self) -> Option<Vec<TierStats>> {
        self.inner
            .tiers
            .as_ref()
            .map(|tiers| tiers.iter().map(|tier| tier.stats.clone()).collect())
    }

    pub async fn publish_to_stream(
        &self,
        stream_key: &str,
        fields: Vec<(String, String)>,
        maxlen: Option<usize>,
    ) -> Result<String> {
        match &self.inner.streaming_backend {
            Some(backend) => backend.stream_add(stream_key, fields, maxlen).await,
            None => Err(anyhow::anyhow!("Streaming backend not configured")),
        }
    }

    pub async fn read_stream_latest(
        &self,
        stream_key: &str,
        count: usize,
    ) -> Result<Vec<(String, Vec<(String, String)>)>> {
        match &self.inner.streaming_backend {
            Some(backend) => backend.stream_read_latest(stream_key, count).await,
            None => Err(anyhow::anyhow!("Streaming backend not configured")),
        }
    }

    pub async fn read_stream(
        &self,
        stream_key: &str,
        last_id: &str,
        count: usize,
        block_ms: Option<usize>,
    ) -> Result<Vec<(String, Vec<(String, String)>)>> {
        match &self.inner.streaming_backend {
            Some(backend) => {
                backend
                    .stream_read(stream_key, last_id, count, block_ms)
                    .await
            }
            None => Err(anyhow::anyhow!("Streaming backend not configured")),
        }
    }

    pub async fn invalidate(&self, key: &str) -> Result<()> {
        if let Some(tiers) = &self.inner.tiers {
            for tier in tiers {
                if let Err(e) = tier.remove(key).await {
                    warn!(
                        "Failed to remove '{}' from L{}: {}",
                        key, tier.tier_level, e
                    );
                }
            }
        } else {
            self.inner.l1_cache.remove(key).await?;
            self.inner.l2_cache.remove(key).await?;
        }

        if let Some(publisher) = &self.inner.invalidation_publisher {
            let mut pub_lock = publisher.lock().await;
            let msg = InvalidationMessage::remove(key);
            pub_lock.publish(&msg).await?;
            self.inner
                .invalidation_stats
                .messages_sent
                .fetch_add(1, Ordering::Relaxed);
        }

        debug!("Invalidated '{}' across all instances", key);
        Ok(())
    }

    pub async fn update_cache<T: Serialize>(
        &self,
        key: &str,
        value: T,
        ttl: Option<Duration>,
    ) -> Result<()> {
        let ttl = ttl.unwrap_or_else(|| CacheStrategy::Default.to_duration());
        let bytes = self.inner.codec.serialize(&value)?;

        if let Some(tiers) = &self.inner.tiers {
            for tier in tiers {
                if let Err(e) = tier.set_with_ttl(key, &bytes, ttl).await {
                    warn!("Failed to update '{}' in L{}: {}", key, tier.tier_level, e);
                }
            }
        } else {
            self.inner.l1_cache.set_with_ttl(key, &bytes, ttl).await?;
            self.inner.l2_cache.set_with_ttl(key, &bytes, ttl).await?;
        }

        if let Some(publisher) = &self.inner.invalidation_publisher {
            let mut pub_lock = publisher.lock().await;
            let value_json = serde_json::to_value(&value).unwrap_or(serde_json::Value::Null);

            let msg = InvalidationMessage::update(key, value_json, Some(ttl));
            pub_lock.publish(&msg).await?;
            self.inner
                .invalidation_stats
                .messages_sent
                .fetch_add(1, Ordering::Relaxed);
        }

        debug!("Updated '{}' across all instances", key);
        Ok(())
    }

    pub async fn invalidate_pattern(&self, pattern: &str) -> Result<()> {
        let keys = if let Some(l2) = &self.inner.l2_cache_concrete {
            l2.scan_keys(pattern).await?
        } else {
            return Err(anyhow::anyhow!(
                "Pattern invalidation requires concrete L2Cache instance"
            ));
        };

        if keys.is_empty() {
            debug!("No keys found matching pattern '{}'", pattern);
            return Ok(());
        }

        if let Some(tiers) = &self.inner.tiers {
            for key in &keys {
                for tier in tiers {
                    if let Err(e) = tier.remove(key).await {
                        warn!(
                            "Failed to remove '{}' from L{}: {}",
                            key, tier.tier_level, e
                        );
                    }
                }
            }
        } else {
            if let Some(l2) = &self.inner.l2_cache_concrete {
                l2.remove_bulk(&keys).await?;
            }
        }

        if let Some(publisher) = &self.inner.invalidation_publisher {
            let mut pub_lock = publisher.lock().await;
            let msg = InvalidationMessage::remove_bulk(keys.clone());
            pub_lock.publish(&msg).await?;
            self.inner
                .invalidation_stats
                .messages_sent
                .fetch_add(1, Ordering::Relaxed);
        }

        debug!(
            "Invalidated {} keys matching pattern '{}'",
            keys.len(),
            pattern
        );
        Ok(())
    }

    pub async fn set_with_broadcast<T: Serialize + Clone>(
        &self,
        key: &str,
        value: T,
        strategy: CacheStrategy,
    ) -> Result<()> {
        let ttl = strategy.to_duration();

        self.set_with_strategy(key, &value, strategy).await?;

        if let Some(publisher) = &self.inner.invalidation_publisher {
            let mut pub_lock = publisher.lock().await;
            let value_json = serde_json::to_value(&value).unwrap_or(serde_json::Value::Null);
            let msg = InvalidationMessage::update(key, value_json, Some(ttl));
            pub_lock.publish(&msg).await?;
            self.inner
                .invalidation_stats
                .messages_sent
                .fetch_add(1, Ordering::Relaxed);
        }

        Ok(())
    }

    pub fn get_invalidation_stats(&self) -> Option<InvalidationStats> {
        if self.inner.invalidation_subscriber.is_some() {
            Some(self.inner.invalidation_stats.snapshot())
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
