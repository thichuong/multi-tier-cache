//! Cache Backend Traits
//!
//! This module defines the trait abstractions that allow users to implement
//! custom cache backends for both L1 (in-memory) and L2 (distributed) caches.
//!
//! # Architecture
//!
//! - `CacheBackend`: Core trait for all cache implementations
//! - `L2CacheBackend`: Extended trait for L2 caches with TTL introspection
//! - `StreamingBackend`: Optional trait for event streaming capabilities
//!
//! # Example: Custom L1 Backend
//!
/// ```rust,no_run
/// use anyhow::Result;
/// use bytes::Bytes;
/// use std::time::Duration;
/// use futures_util::future::BoxFuture;
/// use multi_tier_cache::CacheBackend;
///
/// struct MyCustomCache;
///
/// impl CacheBackend for MyCustomCache {
///     fn get<'a>(&'a self, _key: &'a str) -> BoxFuture<'a, Option<Bytes>> {
///         Box::pin(async move { None })
///     }
///
///     fn set_with_ttl<'a>(&'a self, _key: &'a str, _value: Bytes, _ttl: Duration) -> BoxFuture<'a, Result<()>> {
///         Box::pin(async move { Ok(()) })
///     }
///
///     fn remove<'a>(&'a self, _key: &'a str) -> BoxFuture<'a, Result<()>> {
///         Box::pin(async move { Ok(()) })
///     }
///
///     fn health_check(&self) -> BoxFuture<'_, bool> {
///         Box::pin(async move { true })
///     }
///
///     fn name(&self) -> &'static str { "MyCache" }
/// }
/// ```
use anyhow::Result;
use bytes::Bytes;
use futures_util::future::BoxFuture;
use std::time::Duration;

/// Core cache backend trait for both L1 and L2 caches
///
/// This trait defines the essential operations that any cache backend must support.
/// Implement this trait to create custom L1 (in-memory) or L2 (distributed) cache backends.
///
/// # Required Operations
///
/// - `get`: Retrieve a value by key
/// - `set_with_ttl`: Store a value with a time-to-live
/// - `remove`: Delete a value by key
/// - `health_check`: Verify cache backend is operational
///
/// # Thread Safety
///
/// Implementations must be `Send + Sync` to support concurrent access across async tasks.
///
/// # Performance Considerations
///
/// - `get` operations should be optimized for low latency (target: <1ms for L1, <5ms for L2)
/// - `set_with_ttl` operations can be slightly slower but should still be fast
/// - Consider connection pooling for distributed backends
///
/// # Example
///
/// See module-level documentation for a complete example.
pub trait CacheBackend: Send + Sync {
    /// Get value from cache by key
    ///
    /// # Arguments
    ///
    /// * `key` - The cache key to retrieve
    ///
    /// # Returns
    ///
    /// * `Some(value)` - Value found in cache
    /// * `None` - Key not found or expired
    fn get<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Option<Bytes>>;

    /// Set value in cache with time-to-live
    ///
    /// # Arguments
    ///
    /// * `key` - The cache key
    /// * `value` - The value to store (raw bytes)
    /// * `ttl` - Time-to-live duration
    ///
    /// # Returns
    ///
    /// * `Ok(())` - Value successfully cached
    /// * `Err(e)` - Cache operation failed
    fn set_with_ttl<'a>(
        &'a self,
        key: &'a str,
        value: Bytes,
        ttl: Duration,
    ) -> BoxFuture<'a, Result<()>>;

    /// Remove value from cache
    ///
    /// # Arguments
    ///
    /// * `key` - The cache key to remove
    ///
    /// # Returns
    ///
    /// * `Ok(())` - Value removed (or didn't exist)
    /// * `Err(e)` - Cache operation failed
    fn remove<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Result<()>>;

    /// Check if cache backend is healthy
    ///
    /// This method should verify that the cache backend is operational.
    /// For distributed caches, this typically involves a ping or connectivity check.
    ///
    /// # Returns
    ///
    /// * `true` - Cache is healthy and operational
    /// * `false` - Cache is unhealthy or unreachable
    fn health_check(&self) -> BoxFuture<'_, bool>;

    /// Remove keys matching a pattern
    ///
    /// # Arguments
    ///
    /// * `pattern` - Glob-style pattern (e.g. "user:*")
    ///
    /// # Returns
    ///
    /// * `Ok(())` - Pattern processed
    /// * `Err(e)` - Operation failed
    fn remove_pattern<'a>(&'a self, _pattern: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(async { Ok(()) })
    }

    /// Get the name of this cache backend
    fn name(&self) -> &'static str;
}

// (No longer needed since traits are now dyn-compatible)

/// Extended trait for L2 cache backends with TTL introspection
///
/// This trait extends `CacheBackend` with the ability to retrieve both a value
/// and its remaining TTL. This is essential for implementing efficient L2-to-L1
/// promotion with accurate TTL propagation.
///
/// # Use Cases
///
/// - L2-to-L1 promotion with same TTL
/// - TTL-based cache warming strategies
/// - Monitoring and analytics
///
/// # Example
///
/// ```rust,no_run
/// use anyhow::Result;
/// use bytes::Bytes;
/// use std::time::Duration;
/// use futures_util::future::BoxFuture;
/// use multi_tier_cache::{CacheBackend, L2CacheBackend};
///
/// struct MyDistributedCache;
///
/// impl CacheBackend for MyDistributedCache {
///     fn get<'a>(&'a self, _key: &'a str) -> BoxFuture<'a, Option<Bytes>> { Box::pin(async move { None }) }
///     fn set_with_ttl<'a>(&'a self, _k: &'a str, _v: Bytes, _t: Duration) -> BoxFuture<'a, Result<()>> { Box::pin(async move { Ok(()) }) }
///     fn remove<'a>(&'a self, _k: &'a str) -> BoxFuture<'a, Result<()>> { Box::pin(async move { Ok(()) }) }
///     fn health_check(&self) -> BoxFuture<'_, bool> { Box::pin(async move { true }) }
///     fn name(&self) -> &'static str { "MyDistCache" }
/// }
///
/// impl L2CacheBackend for MyDistributedCache {
///     fn get_with_ttl<'a>(&'a self, _key: &'a str) -> BoxFuture<'a, Option<(Bytes, Option<Duration>)>> {
///         Box::pin(async move { None })
///     }
/// }
/// ```
pub trait L2CacheBackend: CacheBackend {
    /// Get value with its remaining TTL from L2 cache
    fn get_with_ttl<'a>(&'a self, key: &'a str)
    -> BoxFuture<'a, Option<(Bytes, Option<Duration>)>>;
}

// (No longer needed since traits are now dyn-compatible)

/// Optional trait for cache backends that support event streaming
///
/// # Type Definitions
///
/// * `StreamEntry` - A single entry in a stream: `(id, fields)` where fields are `Vec<(key, value)>`
pub type StreamEntry = (String, Vec<(String, String)>);

/// Optional trait for cache backends that support event streaming
///
/// This trait defines operations for event-driven architectures using
/// streaming data structures like Redis Streams.
///
/// # Capabilities
///
/// - Publish events to streams with automatic trimming
/// - Read latest entries (newest first)
/// - Read entries with blocking support
///
/// # Backend Requirements
///
/// Not all cache backends support streaming. This trait is optional and
/// should only be implemented by backends with native streaming support
/// (e.g., Redis Streams, Kafka, Pulsar).
///
/// # Example
///
/// ```rust,no_run
/// use multi_tier_cache::StreamingBackend;
/// use anyhow::Result;
/// use futures_util::future::BoxFuture;
///
/// struct MyStreamingCache;
///
/// impl StreamingBackend for MyStreamingCache {
///     fn stream_add<'a>(
///         &'a self,
///         _stream_key: &'a str,
///         _fields: Vec<(String, String)>,
///         _maxlen: Option<usize>,
///     ) -> BoxFuture<'a, Result<String>> {
///         Box::pin(async move { Ok("entry-id".to_string()) })
///     }
///
///     fn stream_read_latest<'a>(
///         &'a self,
///         _stream_key: &'a str,
///         _count: usize,
///     ) -> BoxFuture<'a, Result<Vec<(String, Vec<(String, String)>)>>> {
///         Box::pin(async move { Ok(vec![]) })
///     }
///
///     fn stream_read<'a>(
///         &'a self,
///         _stream_key: &'a str,
///         _last_id: &'a str,
///         _count: usize,
///         _block_ms: Option<usize>,
///     ) -> BoxFuture<'a, Result<Vec<(String, Vec<(String, String)>)>>> {
///         Box::pin(async move { Ok(vec![]) })
///     }
/// }
/// ```
pub trait StreamingBackend: Send + Sync {
    /// Add an entry to a stream
    fn stream_add<'a>(
        &'a self,
        stream_key: &'a str,
        fields: Vec<(String, String)>,
        maxlen: Option<usize>,
    ) -> BoxFuture<'a, Result<String>>;

    /// Read the latest N entries from a stream (newest first)
    fn stream_read_latest<'a>(
        &'a self,
        stream_key: &'a str,
        count: usize,
    ) -> BoxFuture<'a, Result<Vec<StreamEntry>>>;

    /// Read entries from a stream with optional blocking
    fn stream_read<'a>(
        &'a self,
        stream_key: &'a str,
        last_id: &'a str,
        count: usize,
        block_ms: Option<usize>,
    ) -> BoxFuture<'a, Result<Vec<StreamEntry>>>;
}
