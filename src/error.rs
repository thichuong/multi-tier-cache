use std::fmt;
use thiserror::Error;

#[cfg(feature = "redis")]
use redis::RedisError;

/// Result type for cache operations
pub type CacheResult<T> = std::result::Result<T, CacheError>;

/// Strongly-typed error enum for multi-tier-cache
#[derive(Error, Debug, Clone)]
pub enum CacheError {
    /// Error from a cache backend (Redis, Memcached, etc.)
    BackendError(String),
    /// Error during serialization/deserialization
    SerializationError(String),
    /// Error during cross-instance invalidation
    InvalidationError(String),
    /// Configuration or initialization error
    ConfigError(String),
    /// Key not found in cache
    NotFound,
    /// Internal logic error or unexpected state
    InternalError(String),
}

impl fmt::Display for CacheError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BackendError(msg) => write!(f, "Backend error: {msg}"),
            Self::SerializationError(msg) => write!(f, "Serialization error: {msg}"),
            Self::InvalidationError(msg) => write!(f, "Invalidation error: {msg}"),
            Self::ConfigError(msg) => write!(f, "Configuration error: {msg}"),
            Self::NotFound => write!(f, "Key not found"),
            Self::InternalError(msg) => write!(f, "Internal error: {msg}"),
        }
    }
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
