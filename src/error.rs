use thiserror::Error;

#[cfg(feature = "redis")]
use redis::RedisError;

/// Result type for cache operations
pub type CacheResult<T> = std::result::Result<T, CacheError>;

/// Strongly-typed error enum for multi-tier-cache
#[derive(Error, Debug, Clone)]
pub enum CacheError {
    /// Error from a cache backend (Redis, Memcached, etc.)
    #[error("Backend error: {0}")]
    BackendError(String),
    /// Error during serialization/deserialization
    #[error("Serialization error: {0}")]
    SerializationError(String),
    /// Error during cross-instance invalidation
    #[error("Invalidation error: {0}")]
    InvalidationError(String),
    /// Configuration or initialization error
    #[error("Configuration error: {0}")]
    ConfigError(String),
    /// Key not found in cache
    #[error("Key not found")]
    NotFound,
    /// Internal logic error or unexpected state
    #[error("Internal error: {0}")]
    InternalError(String),
}

#[cfg(feature = "redis")]
impl From<RedisError> for CacheError {
    fn from(err: RedisError) -> Self {
        Self::BackendError(err.to_string())
    }
}

impl From<serde_json::Error> for CacheError {
    fn from(err: serde_json::Error) -> Self {
        Self::SerializationError(err.to_string())
    }
}

#[cfg(feature = "msgpack")]
impl From<rmp_serde::decode::Error> for CacheError {
    fn from(err: rmp_serde::decode::Error) -> Self {
        Self::SerializationError(err.to_string())
    }
}

impl From<std::num::TryFromIntError> for CacheError {
    fn from(err: std::num::TryFromIntError) -> Self {
        Self::ConfigError(err.to_string())
    }
}

#[cfg(feature = "backend-memcached")]
impl From<memcache::MemcacheError> for CacheError {
    fn from(err: memcache::MemcacheError) -> Self {
        Self::BackendError(err.to_string())
    }
}
