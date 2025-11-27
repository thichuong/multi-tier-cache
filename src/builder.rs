//! Cache System Builder
//!
//! Provides a flexible builder pattern for constructing `CacheSystem` with custom backends.
//!
//! # Example: Using Default Backends
//!
//! ```rust,no_run
//! use multi_tier_cache::CacheSystemBuilder;
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let cache = CacheSystemBuilder::new()
//!         .build()
//!         .await?;
//!     Ok(())
//! }
//! ```
//!
//! # Example: Custom L1 Backend
//!
//! ```rust,ignore
//! use multi_tier_cache::{CacheSystemBuilder, CacheBackend};
//! use std::sync::Arc;
//!
//! let custom_l1 = Arc::new(MyCustomL1Cache::new());
//!
//! let cache = CacheSystemBuilder::new()
//!     .with_l1(custom_l1)
//!     .build()
//!     .await?;
//! ```

use crate::traits::{CacheBackend, L2CacheBackend, StreamingBackend};
use crate::{CacheManager, CacheSystem, CacheTier, L1Cache, L2Cache, TierConfig};
use anyhow::Result;
use std::sync::Arc;
use tracing::info;

/// Builder for constructing `CacheSystem` with custom backends
///
/// This builder allows you to configure custom L1 (in-memory) and L2 (distributed)
/// cache backends, enabling you to swap Moka and Redis with alternative implementations.
///
/// # Multi-Tier Support (v0.5.0+)
///
/// The builder now supports dynamic multi-tier architectures (L1+L2+L3+L4+...).
/// Use `.with_tier()` to add custom tiers, or `.with_l3()` / `.with_l4()` for convenience.
///
/// # Default Behavior
///
/// If no custom backends are provided, the builder uses:
/// - **L1**: Moka in-memory cache
/// - **L2**: Redis distributed cache
///
/// # Type Safety
///
/// The builder accepts any type that implements the required traits:
/// - L1 backends must implement `CacheBackend`
/// - L2 backends must implement `L2CacheBackend` (extends `CacheBackend`)
/// - All tier backends must implement `L2CacheBackend` (for TTL support)
/// - Streaming backends must implement `StreamingBackend`
///
/// # Example - Default 2-Tier
///
/// ```rust,no_run
/// use multi_tier_cache::CacheSystemBuilder;
///
/// #[tokio::main]
/// async fn main() -> anyhow::Result<()> {
///     // Use default backends (Moka + Redis)
///     let cache = CacheSystemBuilder::new()
///         .build()
///         .await?;
///
///     Ok(())
/// }
/// ```
///
/// # Example - Custom 3-Tier (v0.5.0+)
///
/// ```rust,ignore
/// use multi_tier_cache::{CacheSystemBuilder, TierConfig};
/// use std::sync::Arc;
///
/// let l1 = Arc::new(L1Cache::new().await?);
/// let l2 = Arc::new(L2Cache::new().await?);
/// let l3 = Arc::new(RocksDBCache::new("/tmp/cache").await?);
///
/// let cache = CacheSystemBuilder::new()
///     .with_tier(l1, TierConfig::as_l1())
///     .with_tier(l2, TierConfig::as_l2())
///     .with_l3(l3)  // Convenience method
///     .build()
///     .await?;
/// ```
pub struct CacheSystemBuilder {
    // Legacy 2-tier configuration (v0.1.0 - v0.4.x)
    l1_backend: Option<Arc<dyn CacheBackend>>,
    l2_backend: Option<Arc<dyn L2CacheBackend>>,
    streaming_backend: Option<Arc<dyn StreamingBackend>>,

    // Multi-tier configuration (v0.5.0+)
    tiers: Vec<(Arc<dyn L2CacheBackend>, TierConfig)>,
}

impl CacheSystemBuilder {
    /// Create a new builder with no custom backends configured
    ///
    /// By default, calling `.build()` will use Moka (L1) and Redis (L2).
    /// Use `.with_tier()` to configure multi-tier architecture (v0.5.0+).
    #[must_use]
    pub fn new() -> Self {
        Self {
            l1_backend: None,
            l2_backend: None,
            streaming_backend: None,
            tiers: Vec::new(),
        }
    }

    /// Configure a custom L1 (in-memory) cache backend
    ///
    /// # Arguments
    ///
    /// * `backend` - Any type implementing `CacheBackend` trait
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use std::sync::Arc;
    /// use multi_tier_cache::CacheSystemBuilder;
    ///
    /// let custom_l1 = Arc::new(MyCustomL1::new());
    ///
    /// let cache = CacheSystemBuilder::new()
    ///     .with_l1(custom_l1)
    ///     .build()
    ///     .await?;
    /// ```
    #[must_use]
    pub fn with_l1(mut self, backend: Arc<dyn CacheBackend>) -> Self {
        self.l1_backend = Some(backend);
        self
    }

    /// Configure a custom L2 (distributed) cache backend
    ///
    /// # Arguments
    ///
    /// * `backend` - Any type implementing `L2CacheBackend` trait
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use std::sync::Arc;
    /// use multi_tier_cache::CacheSystemBuilder;
    ///
    /// let custom_l2 = Arc::new(MyMemcachedBackend::new());
    ///
    /// let cache = CacheSystemBuilder::new()
    ///     .with_l2(custom_l2)
    ///     .build()
    ///     .await?;
    /// ```
    #[must_use]
    pub fn with_l2(mut self, backend: Arc<dyn L2CacheBackend>) -> Self {
        self.l2_backend = Some(backend);
        self
    }

    /// Configure a custom streaming backend
    ///
    /// This is optional. If not provided, streaming functionality will use
    /// the L2 backend if it implements `StreamingBackend`.
    ///
    /// # Arguments
    ///
    /// * `backend` - Any type implementing `StreamingBackend` trait
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use std::sync::Arc;
    /// use multi_tier_cache::CacheSystemBuilder;
    ///
    /// let kafka_backend = Arc::new(MyKafkaBackend::new());
    ///
    /// let cache = CacheSystemBuilder::new()
    ///     .with_streams(kafka_backend)
    ///     .build()
    ///     .await?;
    /// ```
    #[must_use]
    pub fn with_streams(mut self, backend: Arc<dyn StreamingBackend>) -> Self {
        self.streaming_backend = Some(backend);
        self
    }

    /// Configure a cache tier with custom settings (v0.5.0+)
    ///
    /// Add a cache tier to the multi-tier architecture. Tiers will be sorted
    /// by `tier_level` during build.
    ///
    /// # Arguments
    ///
    /// * `backend` - Any type implementing `L2CacheBackend` trait
    /// * `config` - Tier configuration (level, promotion, TTL scale)
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use multi_tier_cache::{CacheSystemBuilder, TierConfig, L1Cache, L2Cache};
    /// use std::sync::Arc;
    ///
    /// let l1 = Arc::new(L1Cache::new().await?);
    /// let l2 = Arc::new(L2Cache::new().await?);
    /// let l3 = Arc::new(RocksDBCache::new("/tmp").await?);
    ///
    /// let cache = CacheSystemBuilder::new()
    ///     .with_tier(l1, TierConfig::as_l1())
    ///     .with_tier(l2, TierConfig::as_l2())
    ///     .with_tier(l3, TierConfig::as_l3())
    ///     .build()
    ///     .await?;
    /// ```
    #[must_use]
    pub fn with_tier(mut self, backend: Arc<dyn L2CacheBackend>, config: TierConfig) -> Self {
        self.tiers.push((backend, config));
        self
    }

    /// Convenience method to add L3 cache tier (v0.5.0+)
    ///
    /// Adds a cold storage tier with 2x TTL multiplier.
    ///
    /// # Arguments
    ///
    /// * `backend` - L3 backend (e.g., `RocksDB`, `LevelDB`)
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use std::sync::Arc;
    ///
    /// let rocksdb = Arc::new(RocksDBCache::new("/tmp/l3cache").await?);
    ///
    /// let cache = CacheSystemBuilder::new()
    ///     .with_l3(rocksdb)
    ///     .build()
    ///     .await?;
    /// ```
    #[must_use]
    pub fn with_l3(mut self, backend: Arc<dyn L2CacheBackend>) -> Self {
        self.tiers.push((backend, TierConfig::as_l3()));
        self
    }

    /// Convenience method to add L4 cache tier (v0.5.0+)
    ///
    /// Adds an archive storage tier with 8x TTL multiplier.
    ///
    /// # Arguments
    ///
    /// * `backend` - L4 backend (e.g., S3, file system)
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use std::sync::Arc;
    ///
    /// let s3_cache = Arc::new(S3Cache::new("my-bucket").await?);
    ///
    /// let cache = CacheSystemBuilder::new()
    ///     .with_l4(s3_cache)
    ///     .build()
    ///     .await?;
    /// ```
    #[must_use]
    pub fn with_l4(mut self, backend: Arc<dyn L2CacheBackend>) -> Self {
        self.tiers.push((backend, TierConfig::as_l4()));
        self
    }

    /// Build the `CacheSystem` with configured or default backends
    ///
    /// If no custom backends were provided via `.with_l1()` or `.with_l2()`,
    /// this method creates default backends (Moka for L1, Redis for L2).
    ///
    /// # Multi-Tier Mode (v0.5.0+)
    ///
    /// If tiers were configured via `.with_tier()`, `.with_l3()`, or `.with_l4()`,
    /// the builder creates a multi-tier `CacheManager` using `new_with_tiers()`.
    ///
    /// # Returns
    ///
    /// * `Ok(CacheSystem)` - Successfully constructed cache system
    /// * `Err(e)` - Failed to initialize backends (e.g., Redis connection error)
    ///
    /// # Example - Default 2-Tier
    ///
    /// ```rust,no_run
    /// use multi_tier_cache::CacheSystemBuilder;
    ///
    /// #[tokio::main]
    /// async fn main() -> anyhow::Result<()> {
    ///     let cache = CacheSystemBuilder::new()
    ///         .build()
    ///         .await?;
    ///
    ///     // Use cache_manager for operations
    ///     let manager = cache.cache_manager();
    ///
    ///     Ok(())
    /// }
    /// ```
    /// # Errors
    ///
    /// Returns an error if the default backends cannot be initialized.
    pub async fn build(self) -> Result<CacheSystem> {
        info!("Building Multi-Tier Cache System");

        // NEW: Multi-tier mode (v0.5.0+)
        if !self.tiers.is_empty() {
            info!(
                tier_count = self.tiers.len(),
                "Initializing multi-tier architecture"
            );

            // Sort tiers by tier_level (ascending: L1 first, L4 last)
            let mut tiers = self.tiers;
            tiers.sort_by_key(|(_, config)| config.tier_level);

            // Convert to CacheTier instances
            let cache_tiers: Vec<CacheTier> = tiers
                .into_iter()
                .map(|(backend, config)| {
                    CacheTier::new(
                        backend,
                        config.tier_level,
                        config.promotion_enabled,
                        config.ttl_scale,
                    )
                })
                .collect();

            // Create cache manager with multi-tier support
            let cache_manager = Arc::new(CacheManager::new_with_tiers(
                cache_tiers,
                self.streaming_backend,
            )?);

            info!("Multi-Tier Cache System built successfully");
            info!("Note: Using multi-tier mode - use cache_manager() for all operations");

            return Ok(CacheSystem {
                cache_manager,
                l1_cache: None, // Multi-tier mode doesn't use concrete types
                l2_cache: None,
            });
        }

        // LEGACY: 2-tier mode (v0.1.0 - v0.4.x)
        // Handle default vs custom backends
        if self.l1_backend.is_none() && self.l2_backend.is_none() {
            // Default path: Create concrete types once and reuse them
            info!("Initializing default backends (Moka + Redis)");

            let l1_cache = Arc::new(L1Cache::new()?);
            let l2_cache = Arc::new(L2Cache::new().await?);

            // Use legacy constructor that handles conversion to trait objects
            let cache_manager =
                Arc::new(CacheManager::new(l1_cache.clone(), l2_cache.clone()).await?);

            info!("Multi-Tier Cache System built successfully");

            Ok(CacheSystem {
                cache_manager,
                l1_cache: Some(l1_cache),
                l2_cache: Some(l2_cache),
            })
        } else {
            // Custom backend path
            info!("Building with custom backends");

            let l1_backend: Arc<dyn CacheBackend> = if let Some(backend) = self.l1_backend {
                info!(backend = %backend.name(), "Using custom L1 backend");
                backend
            } else {
                info!("Using default L1 backend (Moka)");
                Arc::new(L1Cache::new()?)
            };

            let l2_backend: Arc<dyn L2CacheBackend> = if let Some(backend) = self.l2_backend {
                info!(backend = %backend.name(), "Using custom L2 backend");
                backend
            } else {
                info!("Using default L2 backend (Redis)");
                Arc::new(L2Cache::new().await?)
            };

            let streaming_backend = self.streaming_backend;

            // Create cache manager with trait objects
            let cache_manager = Arc::new(CacheManager::new_with_backends(
                l1_backend,
                l2_backend,
                streaming_backend,
            )?);

            info!("Multi-Tier Cache System built with custom backends");
            info!("Note: Using custom backends - use cache_manager() for all operations");

            Ok(CacheSystem {
                cache_manager,
                l1_cache: None, // Custom backends mode doesn't use concrete types
                l2_cache: None,
            })
        }
    }
}

impl Default for CacheSystemBuilder {
    fn default() -> Self {
        Self::new()
    }
}
