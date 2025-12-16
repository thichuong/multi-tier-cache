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
//!         .with_l1(custom_l1)
//!         .build()
//!         .await?;
//! ```

use crate::backends::MokaCacheConfig;
use crate::codecs::JsonCodec;
use crate::traits::{CacheBackend, CacheCodec, L2CacheBackend, StreamingBackend};
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
pub struct CacheSystemBuilder<C: CacheCodec = JsonCodec> {
    // Legacy 2-tier configuration (v0.1.0 - v0.4.x)
    l1_backend: Option<Arc<dyn CacheBackend>>,
    l2_backend: Option<Arc<dyn L2CacheBackend>>,

    streaming_backend: Option<Arc<dyn StreamingBackend>>,
    moka_config: Option<MokaCacheConfig>,

    // Multi-tier configuration (v0.5.0+)
    tiers: Vec<(Arc<dyn L2CacheBackend>, TierConfig)>,

    // Codec
    codec: C,
}

impl CacheSystemBuilder<JsonCodec> {
    /// Create a new builder with no custom backends configured
    ///
    /// By default, calling `.build()` will use Moka (L1) and Redis (L2) with JsonCodec.
    #[must_use]
    pub fn new() -> Self {
        Self {
            l1_backend: None,
            l2_backend: None,
            streaming_backend: None,
            moka_config: None,
            tiers: Vec::new(),
            codec: JsonCodec::default(),
        }
    }
}

impl<C: CacheCodec + Clone + 'static> CacheSystemBuilder<C> {
    /// Configure a custom L1 (in-memory) cache backend
    #[must_use]
    pub fn with_l1(mut self, backend: Arc<dyn CacheBackend>) -> Self {
        self.l1_backend = Some(backend);
        self
    }

    /// Configure custom configuration for default L1 (Moka) backend
    #[must_use]
    pub fn with_moka_config(mut self, config: MokaCacheConfig) -> Self {
        self.moka_config = Some(config);
        self
    }

    /// Configure a custom L2 (distributed) cache backend
    #[must_use]
    pub fn with_l2(mut self, backend: Arc<dyn L2CacheBackend>) -> Self {
        self.l2_backend = Some(backend);
        self
    }

    /// Configure a custom streaming backend
    #[must_use]
    pub fn with_streams(mut self, backend: Arc<dyn StreamingBackend>) -> Self {
        self.streaming_backend = Some(backend);
        self
    }

    /// Configure a cache tier with custom settings (v0.5.0+)
    #[must_use]
    pub fn with_tier(mut self, backend: Arc<dyn L2CacheBackend>, config: TierConfig) -> Self {
        self.tiers.push((backend, config));
        self
    }

    /// Convenience method to add L3 cache tier (v0.5.0+)
    #[must_use]
    pub fn with_l3(mut self, backend: Arc<dyn L2CacheBackend>) -> Self {
        self.tiers.push((backend, TierConfig::as_l3()));
        self
    }

    /// Convenience method to add L4 cache tier (v0.5.0+)
    #[must_use]
    pub fn with_l4(mut self, backend: Arc<dyn L2CacheBackend>) -> Self {
        self.tiers.push((backend, TierConfig::as_l4()));
        self
    }

    /// Set the serialization codec
    pub fn with_codec<NC: CacheCodec>(self, codec: NC) -> CacheSystemBuilder<NC> {
        CacheSystemBuilder {
            l1_backend: self.l1_backend,
            l2_backend: self.l2_backend,
            streaming_backend: self.streaming_backend,
            moka_config: self.moka_config,
            tiers: self.tiers,
            codec,
        }
    }

    /// Build the `CacheSystem` with configured or default backends
    pub async fn build(self) -> Result<CacheSystem<C>> {
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
            let cache_manager = Arc::new(CacheManager::with_tiers_and_codec(
                cache_tiers,
                self.streaming_backend,
                self.codec,
            )?);

            info!("Multi-Tier Cache System built successfully");
            info!("Note: Using multi-tier mode - use cache_manager() for all operations");

            return Ok(CacheSystem {
                cache_manager,
                l1_cache: None,
                l2_cache: None,
            });
        }

        // LEGACY: 2-tier mode (v0.1.0 - v0.4.x)
        // Handle default vs custom backends
        if self.l1_backend.is_none() && self.l2_backend.is_none() {
            // Default path: Create concrete types once and reuse them
            info!("Initializing default backends (Moka + Redis)");

            let l1_cache = Arc::new(L1Cache::new(self.moka_config.unwrap_or_default())?);
            let l2_cache = Arc::new(L2Cache::new().await?);

            // Use legacy constructor that handles conversion to trait objects
            // But we must use with_codec since C might not be JsonCodec (though it defaults to it)
            // Wait, we can convert concrete backends to trait objects manually here
            let l1_backend: Arc<dyn CacheBackend> = l1_cache.clone();
            let l2_backend: Arc<dyn L2CacheBackend> = l2_cache.clone();

            // Should we set streaming default?
            let redis_url =
                std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());
            let redis_streams = crate::redis_streams::RedisStreams::new(&redis_url).await?;
            let streaming_backend: Arc<dyn StreamingBackend> = Arc::new(redis_streams);

            let cache_manager = Arc::new(CacheManager::with_codec(
                l1_backend,
                l2_backend,
                Some(streaming_backend),
                self.codec,
            )?);

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
                let config = self.moka_config.unwrap_or_default();
                Arc::new(L1Cache::new(config)?)
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
            let cache_manager = Arc::new(CacheManager::with_codec(
                l1_backend,
                l2_backend,
                streaming_backend,
                self.codec,
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
