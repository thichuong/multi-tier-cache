//! Common utilities for integration tests
//!
//! This module provides shared test infrastructure including:
//! - Redis connection helpers
//! - Test data generators
//! - Cleanup utilities
//! - Test environment setup

use anyhow::Result;
use multi_tier_cache::{CacheManager, CacheSystem, InvalidationConfig, L1Cache, L2Cache};
use std::sync::Arc;

/// Get Redis URL from environment or use default
pub fn redis_url() -> String {
    std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string())
}

/// Generate a unique test key prefix to avoid conflicts between tests
pub fn test_key_prefix() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis();
    format!("test:{}:", timestamp)
}

/// Create a test key with unique prefix
pub fn test_key(name: &str) -> String {
    format!("test_{}_{}", name, rand::random::<u32>())
}

/// Initialize a basic cache system for testing
pub async fn setup_cache_system() -> Result<CacheSystem> {
    std::env::set_var("REDIS_URL", redis_url());
    CacheSystem::new().await
}

/// Initialize cache manager with invalidation for testing
pub async fn setup_cache_with_invalidation() -> Result<Arc<CacheManager>> {
    let l1 = Arc::new(L1Cache::new().await?);
    let l2 = Arc::new(L2Cache::new().await?);
    let config = InvalidationConfig::default();

    let manager = CacheManager::new_with_invalidation(l1, l2, &redis_url(), config).await?;

    Ok(Arc::new(manager))
}

/// Cleanup test keys from Redis
pub async fn cleanup_test_keys(prefix: &str) -> Result<()> {
    let cache = setup_cache_system().await?;
    let l2 = Arc::new(L2Cache::new().await?);

    // Find all test keys
    let pattern = format!("{}*", prefix);
    let keys = l2.scan_keys(&pattern).await?;

    // Remove them
    if !keys.is_empty() {
        l2.remove_bulk(&keys).await?;
    }

    Ok(())
}

/// Generate test data of various types
pub mod test_data {
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    pub struct User {
        pub id: u64,
        pub name: String,
        pub email: String,
    }

    impl User {
        pub fn new(id: u64) -> Self {
            Self {
                id,
                name: format!("User {}", id),
                email: format!("user{}@example.com", id),
            }
        }
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    pub struct Product {
        pub id: u64,
        pub name: String,
        pub price: f64,
        pub category: String,
    }

    impl Product {
        pub fn new(id: u64) -> Self {
            Self {
                id,
                name: format!("Product {}", id),
                price: 99.99 + (id as f64),
                category: format!("Category {}", id % 5),
            }
        }
    }

    /// Generate JSON test data
    pub fn json_user(id: u64) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "name": format!("User {}", id),
            "email": format!("user{}@example.com", id),
            "created_at": "2025-01-01T00:00:00Z"
        })
    }

    /// Generate JSON test data with specified size
    pub fn json_data_sized(size_kb: usize) -> serde_json::Value {
        let data_string = "x".repeat(size_kb * 1024);
        serde_json::json!({
            "data": data_string,
            "size_kb": size_kb
        })
    }
}

/// Wait for a condition with timeout
pub async fn wait_for<F>(mut condition: F, timeout_ms: u64) -> bool
where
    F: FnMut() -> bool,
{
    use tokio::time::{sleep, Duration};

    let start = std::time::Instant::now();
    let timeout = Duration::from_millis(timeout_ms);

    while start.elapsed() < timeout {
        if condition() {
            return true;
        }
        sleep(Duration::from_millis(10)).await;
    }

    false
}

/// Assert that cache stats meet expectations
#[macro_export]
macro_rules! assert_cache_stats {
    ($cache:expr, $field:ident > $value:expr) => {
        let stats = $cache.cache_manager().get_stats();
        assert!(
            stats.$field > $value,
            "Expected {} > {}, got {}",
            stringify!($field),
            $value,
            stats.$field
        );
    };
    ($cache:expr, $field:ident == $value:expr) => {
        let stats = $cache.cache_manager().get_stats();
        assert_eq!(
            stats.$field,
            $value,
            "Expected {} == {}, got {}",
            stringify!($field),
            $value,
            stats.$field
        );
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_key_generation() {
        let key1 = test_key("user");
        let key2 = test_key("user");
        assert_ne!(key1, key2, "Keys should be unique");
        assert!(key1.starts_with("test_user_"));
    }

    #[test]
    fn test_data_generation() {
        let user = test_data::User::new(123);
        assert_eq!(user.id, 123);
        assert_eq!(user.name, "User 123");
        assert_eq!(user.email, "user123@example.com");
    }
}
