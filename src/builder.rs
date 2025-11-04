//! Cache System Builder
//!
//! Provides a flexible builder pattern for constructing CacheSystem with custom backends.
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

use std::sync::Arc;
use anyhow::Result;
use crate::traits::{CacheBackend, L2CacheBackend, StreamingBackend};
use crate::{CacheManager, L1Cache, L2Cache, CacheSystem};

/// Builder for constructing CacheSystem with custom backends
///
/// This builder allows you to configure custom L1 (in-memory) and L2 (distributed)
/// cache backends, enabling you to swap Moka and Redis with alternative implementations.
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
/// - Streaming backends must implement `StreamingBackend`
///
/// # Example
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
pub struct CacheSystemBuilder {
    l1_backend: Option<Arc<dyn CacheBackend>>,
    l2_backend: Option<Arc<dyn L2CacheBackend>>,
    streaming_backend: Option<Arc<dyn StreamingBackend>>,
}

impl CacheSystemBuilder {
    /// Create a new builder with no custom backends configured
    ///
    /// By default, calling `.build()` will use Moka (L1) and Redis (L2).
    pub fn new() -> Self {
        Self {
            l1_backend: None,
            l2_backend: None,
            streaming_backend: None,
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
    pub fn with_streams(mut self, backend: Arc<dyn StreamingBackend>) -> Self {
        self.streaming_backend = Some(backend);
        self
    }

    /// Build the CacheSystem with configured or default backends
    ///
    /// If no custom backends were provided via `.with_l1()` or `.with_l2()`,
    /// this method creates default backends (Moka for L1, Redis for L2).
    ///
    /// # Returns
    ///
    /// * `Ok(CacheSystem)` - Successfully constructed cache system
    /// * `Err(e)` - Failed to initialize backends (e.g., Redis connection error)
    ///
    /// # Note
    ///
    /// Currently, custom backends are stored but not yet used by CacheManager.
    /// Full trait-based backend support will be available in the next phase.
    /// For now, this builder ensures API stability for future upgrades.
    ///
    /// # Example
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
    pub async fn build(self) -> Result<CacheSystem> {
        println!("🏗️ Building Multi-Tier Cache System...");

        // Handle default vs custom backends
        if self.l1_backend.is_none() && self.l2_backend.is_none() {
            // Default path: Create concrete types once and reuse them
            println!("  🚀 Initializing default backends (Moka + Redis)...");

            let l1_cache = Arc::new(L1Cache::new().await?);
            let l2_cache = Arc::new(L2Cache::new().await?);

            // Use legacy constructor that handles conversion to trait objects
            let cache_manager = Arc::new(CacheManager::new(l1_cache.clone(), l2_cache.clone()).await?);

            println!("✅ Multi-Tier Cache System built successfully");

            Ok(CacheSystem {
                cache_manager,
                l1_cache,
                l2_cache,
            })
        } else {
            // Custom backend path
            println!("  ℹ️ Building with custom backends...");

            let l1_backend: Arc<dyn CacheBackend> = match self.l1_backend {
                Some(backend) => {
                    println!("  ✅ Using custom L1 backend: {}", backend.name());
                    backend
                }
                None => {
                    println!("  🚀 Using default L1 backend (Moka)");
                    Arc::new(L1Cache::new().await?)
                }
            };

            let l2_backend: Arc<dyn L2CacheBackend> = match self.l2_backend {
                Some(backend) => {
                    println!("  ✅ Using custom L2 backend: {}", backend.name());
                    backend
                }
                None => {
                    println!("  🔴 Using default L2 backend (Redis)");
                    Arc::new(L2Cache::new().await?)
                }
            };

            let streaming_backend = self.streaming_backend;

            // Create cache manager with trait objects
            let cache_manager = Arc::new(
                CacheManager::new_with_backends(
                    l1_backend,
                    l2_backend,
                    streaming_backend,
                ).await?
            );

            // Create placeholder concrete types for backward compatibility
            // Note: These are NOT the same instances as used by cache_manager
            // Users must use cache_manager() for all operations
            let l1_placeholder = Arc::new(L1Cache::new().await?);
            let l2_placeholder = Arc::new(L2Cache::new().await?);

            println!("✅ Multi-Tier Cache System built with custom backends");
            println!("  ⚠️ Note: Direct l1_cache/l2_cache fields are placeholders");
            println!("  ⚠️ Use cache_manager() for all cache operations");

            Ok(CacheSystem {
                cache_manager,
                l1_cache: l1_placeholder,
                l2_cache: l2_placeholder,
            })
        }
    }
}

impl Default for CacheSystemBuilder {
    fn default() -> Self {
        Self::new()
    }
}
