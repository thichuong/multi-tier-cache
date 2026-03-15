//! Multi-Tier Cache
//!
//! A high-performance, production-ready multi-tier caching library for Rust featuring:
//! - **L1 Cache**: In-memory caching with Moka (sub-millisecond latency)
//! - **L2 Cache**: Distributed caching with Redis (persistent storage)
//! - **Cache Stampede Protection**: `DashMap` + Mutex request coalescing
//! - **Redis Streams**: Built-in support for event streaming
//! - **Automatic L2-to-L1 Promotion**: Intelligent cache tier promotion
//! - **Comprehensive Statistics**: Hit rates, promotions, in-flight tracking
//!
//! # Quick Start
//!
//! ```rust,no_run
//! use multi_tier_cache::{CacheSystem, CacheStrategy};
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     // Initialize cache system
//!     let cache = CacheSystem::new().await?;
//!
//!     // Store data with cache strategy
//!     let data = bytes::Bytes::from("{\"user\": \"alice\", \"score\": 100}");
//!     cache.cache_manager()
//!         .set_with_strategy("user:1", data, CacheStrategy::ShortTerm)
//!         .await?;
//!
//!     // Retrieve data (L1 first, then L2 fallback)
//!     if let Some(cached) = cache.cache_manager().get("user:1").await? {
//!         tracing::info!("Cached data: {:?}", cached);
//!     }
//!
//!     // Get statistics
//!     let stats = cache.cache_manager().get_stats();
//!     tracing::info!("Hit rate: {:.2}%", stats.hit_rate);
//!
//!     Ok(())
//! }
//! ```
//!
//! # Features
//!
//! - **Multi-Tier Architecture**: Combines fast in-memory (L1) with persistent distributed (L2) caching
//! - **Cache Stampede Protection**: Prevents duplicate computations during cache misses
//! - **Redis Streams**: Publish/subscribe with automatic trimming
//! - **Zero-Config**: Sensible defaults, works out of the box
//! - **Production-Proven**: Battle-tested at 16,829+ RPS with 5.2ms latency
//!
//! # Architecture
//!
//! ```text
//! Request → L1 Cache (Moka) → L2 Cache (Redis) → Compute/Fetch
//!           ↓ Hit (90%)       ↓ Hit (75%)        ↓ Miss (5%)
//!           Return            Promote to L1       Store in L1+L2
//! ```

pub mod backends;
pub mod builder;
pub mod cache_manager;
pub mod error;
#[cfg(feature = "redis")]
pub mod invalidation;
#[cfg(feature = "redis")]
pub mod redis_streams;
pub mod serialization;
pub mod traits;

pub use error::{CacheError, CacheResult};
pub use serialization::{CacheSerializer, JsonSerializer};
pub use std::sync::Arc;
use tracing::{info, warn};

// Re-export backend types (maintains backward compatibility)
pub use backends::DashMapCache;
#[cfg(feature = "moka")]
pub use backends::{L1Cache, MokaCache, MokaCacheConfig};
#[cfg(feature = "redis")]
pub use backends::{L2Cache, RedisCache};

// Optional backends (feature-gated)
#[cfg(feature = "backend-memcached")]
pub use backends::MemcachedCache;

#[cfg(feature = "quick_cache")]
pub use backends::QuickCacheBackend;
pub use builder::CacheSystemBuilder;
pub use bytes::Bytes;
pub use cache_manager::{
    CacheManager,
    CacheManagerStats,
    CacheStrategy,
    CacheTier,
    // Multi-tier support (v0.5.0+)
    TierConfig,
    TierStats,
};
#[cfg(feature = "redis")]
pub use invalidation::{
    InvalidationConfig, InvalidationMessage, InvalidationPublisher, InvalidationStats,
    InvalidationSubscriber, ReliableStreamSubscriber,
};
#[cfg(feature = "redis")]
pub use redis_streams::RedisStreams;
pub use traits::{CacheBackend, L2CacheBackend, StreamingBackend};

// Re-export backend types (maintains backward compatibility)

/// Main entry point for the Multi-Tier Cache system
///
/// Provides unified access to L1 (Moka) and L2 (Redis) caches with
/// automatic failover, promotion, and stampede protection.
///
/// # Example
///
/// ```rust,no_run
/// use multi_tier_cache::CacheSystem;
///
/// #[tokio::main]
/// async fn main() -> anyhow::Result<()> {
///     let cache = CacheSystem::new().await?;
///
///     // Use cache_manager for all operations
///     let manager = cache.cache_manager();
///
///     Ok(())
/// }
/// ```
///
/// # Note on `l1_cache` and `l2_cache` Fields
///
/// When using multi-tier mode or custom backends, `l1_cache` and `l2_cache`
/// may be `None`. Always use `cache_manager()` for cache operations.
#[derive(Clone)]
pub struct CacheSystem {
    /// Unified cache manager (primary interface)
    pub cache_manager: Arc<CacheManager>,
    /// Optional L1 cache instance (for health checks)
    #[cfg(feature = "moka")]
    pub l1_cache: Option<Arc<L1Cache>>,
    /// Optional L2 cache instance (for health checks)
    #[cfg(feature = "redis")]
    pub l2_cache: Option<Arc<L2Cache>>,
}

impl CacheSystem {
    /// Create new cache system with default configuration
    ///
    /// # Configuration
    ///
    /// Redis connection is configured via `REDIS_URL` environment variable.
    /// Default: `redis://127.0.0.1:6379`
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use multi_tier_cache::CacheSystem;
    ///
    /// #[tokio::main]
    /// async fn main() -> anyhow::Result<()> {
    ///     // Set environment variable (optional)
    ///     unsafe { std::env::set_var("REDIS_URL", "redis://localhost:6379") };
    ///
    ///     let cache = CacheSystem::new().await?;
    ///     Ok(())
    /// }
    /// ```
    /// # Errors
    ///
    /// Returns an error if cache initialization fails.
    #[cfg(all(feature = "moka", feature = "redis"))]
    pub async fn new() -> CacheResult<Self> {
        info!("Initializing Multi-Tier Cache System");

        // Initialize L1 cache (Moka)
        let l1_cache = Arc::new(L1Cache::new(MokaCacheConfig::default())?);

        // Initialize L2 cache (Redis)
        let l2_cache = Arc::new(L2Cache::new().await?);

        // Initialize cache manager
        let cache_manager = Arc::new(CacheManager::new(l1_cache.clone(), l2_cache.clone()).await?);

        info!("Multi-Tier Cache System initialized successfully");

        Ok(Self {
            cache_manager,
            l1_cache: Some(l1_cache),
            l2_cache: Some(l2_cache),
        })
    }

    /// Create cache system with custom Redis URL
    ///
    /// # Arguments
    ///
    /// * `redis_url` - Redis connection string (e.g., `<redis://localhost:6379>`)
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use multi_tier_cache::CacheSystem;
    ///
    /// #[tokio::main]
    /// async fn main() -> anyhow::Result<()> {
    ///     let cache = CacheSystem::with_redis_url("redis://custom:6379").await?;
    ///     Ok(())
    /// }
    /// ```
    /// # Errors
    ///
    /// Returns an error if cache initialization fails.
    #[cfg(all(feature = "moka", feature = "redis"))]
    pub async fn with_redis_url(redis_url: &str) -> CacheResult<Self> {
        info!(redis_url = %redis_url, "Initializing Multi-Tier Cache System with custom Redis URL");

        // Initialize L1 cache (Moka)
        let l1_cache = Arc::new(L1Cache::new(MokaCacheConfig::default())?);

        // Initialize L2 cache (Redis) with custom URL
        let l2_cache = Arc::new(L2Cache::with_url(redis_url).await?);

        // Initialize cache manager
        let cache_manager =
            Arc::new(CacheManager::new(Arc::clone(&l1_cache), Arc::clone(&l2_cache)).await?);

        info!("Multi-Tier Cache System initialized successfully");

        Ok(Self {
            cache_manager,
            l1_cache: Some(l1_cache),
            l2_cache: Some(l2_cache),
        })
    }

    /// Perform health check on all cache tiers
    ///
    /// Returns `true` if at least L1 is operational.
    /// L2 failure is tolerated (graceful degradation).
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use multi_tier_cache::CacheSystem;
    ///
    /// #[tokio::main]
    /// async fn main() -> anyhow::Result<()> {
    ///     let cache = CacheSystem::new().await?;
    ///
    ///     if cache.health_check().await {
    ///         tracing::info!("Cache system healthy");
    ///     }
    ///
    ///     Ok(())
    /// }
    /// ```
    pub async fn health_check(&self) -> bool {
        #[cfg(feature = "moka")]
        let l1_ok = match &self.l1_cache {
            Some(cache) => {
                let backend: &dyn crate::traits::CacheBackend = cache.as_ref();
                backend.health_check().await
            }
            None => true,
        };
        #[cfg(not(feature = "moka"))]
        let l1_ok = true;

        #[cfg(feature = "redis")]
        let l2_ok = match &self.l2_cache {
            Some(cache) => {
                let backend: &dyn crate::traits::CacheBackend = cache.as_ref();
                backend.health_check().await
            }
            None => true,
        };
        #[cfg(not(feature = "redis"))]
        let l2_ok = true;

        if l1_ok && l2_ok {
            info!("Multi-Tier Cache health check passed");
            true
        } else {
            warn!(l1_ok = %l1_ok, l2_ok = %l2_ok, "Multi-Tier Cache health check - partial failure");
            l1_ok // At minimum, L1 should work
        }
    }

    /// Get reference to cache manager (primary interface)
    ///
    /// Use this for all cache operations: get, set, streams, etc.
    #[must_use]
    pub fn cache_manager(&self) -> &Arc<CacheManager> {
        &self.cache_manager
    }
}
