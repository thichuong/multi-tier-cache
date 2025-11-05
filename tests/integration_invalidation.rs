//! Integration tests for cache invalidation
//!
//! Tests cross-instance cache invalidation via Redis Pub/Sub

mod common;

use common::*;
use multi_tier_cache::CacheStrategy;
use std::time::Duration;
use tokio::time::sleep;

/// Test single key invalidation
#[tokio::test]
async fn test_invalidate_single_key() {
    let cache = setup_cache_with_invalidation().await.unwrap();
    let key = test_key("invalidate");
    let value = test_data::json_user(1);

    // Set value
    cache
        .set_with_strategy(&key, value.clone(), CacheStrategy::MediumTerm)
        .await
        .unwrap();

    // Verify cached
    let cached = cache.get(&key).await.unwrap();
    assert_eq!(cached, Some(value));

    // Invalidate
    cache.invalidate(&key).await.unwrap();

    // Should be gone
    let cached2 = cache.get(&key).await.unwrap();
    assert_eq!(cached2, None);
}

/// Test update cache across instances
#[tokio::test]
async fn test_update_cache() {
    let cache = setup_cache_with_invalidation().await.unwrap();
    let key = test_key("update");
    let value1 = test_data::json_user(1);
    let value2 = test_data::json_user(2);

    // Set initial value
    cache
        .set_with_strategy(&key, value1.clone(), CacheStrategy::MediumTerm)
        .await
        .unwrap();

    // Update cache
    cache
        .update_cache(&key, value2.clone(), Some(Duration::from_secs(300)))
        .await
        .unwrap();

    // Wait for pub/sub propagation
    sleep(Duration::from_millis(100)).await;

    // Should have new value
    let cached = cache.get(&key).await.unwrap();
    assert_eq!(cached, Some(value2));
}

/// Test pattern-based invalidation
#[tokio::test]
async fn test_invalidate_pattern() {
    let cache = setup_cache_with_invalidation().await.unwrap();
    let prefix = format!("user:pattern:{}:", rand::random::<u32>());

    // Set multiple keys with same prefix
    for i in 1..=5 {
        let key = format!("{}key{}", prefix, i);
        let value = test_data::json_user(i);
        cache
            .set_with_strategy(&key, value, CacheStrategy::MediumTerm)
            .await
            .unwrap();
    }

    // Invalidate by pattern
    let pattern = format!("{}*", prefix);
    cache.invalidate_pattern(&pattern).await.unwrap();

    // Wait for propagation
    sleep(Duration::from_millis(100)).await;

    // All should be gone
    for i in 1..=5 {
        let key = format!("{}key{}", prefix, i);
        let cached = cache.get(&key).await.unwrap();
        assert_eq!(cached, None, "Key {} should be invalidated", key);
    }
}

/// Test write-through broadcast
#[tokio::test]
async fn test_set_with_broadcast() {
    let cache = setup_cache_with_invalidation().await.unwrap();
    let key = test_key("broadcast");
    let value = test_data::json_user(3);

    // Set with broadcast
    cache
        .set_with_broadcast(&key, value.clone(), CacheStrategy::ShortTerm)
        .await
        .unwrap();

    // Wait for propagation
    sleep(Duration::from_millis(100)).await;

    // Should be cached
    let cached = cache.get(&key).await.unwrap();
    assert_eq!(cached, Some(value));
}

/// Test invalidation statistics
#[tokio::test]
async fn test_invalidation_stats() {
    let cache = setup_cache_with_invalidation().await.unwrap();
    let key = test_key("stats");
    let value = test_data::json_user(4);

    // Perform invalidation operations
    cache
        .set_with_strategy(&key, value.clone(), CacheStrategy::ShortTerm)
        .await
        .unwrap();

    cache.invalidate(&key).await.unwrap();

    // Check stats
    if let Some(stats) = cache.get_invalidation_stats() {
        assert!(stats.messages_sent >= 1, "Should have sent invalidation messages");
    } else {
        panic!("Invalidation stats should be available");
    }
}

/// Test multiple invalidation operations
#[tokio::test]
async fn test_bulk_invalidation() {
    let cache = setup_cache_with_invalidation().await.unwrap();

    // Set multiple keys
    let keys: Vec<String> = (1..=10).map(|i| test_key(&format!("bulk{}", i))).collect();

    for (i, key) in keys.iter().enumerate() {
        let value = test_data::json_user((i + 1) as u64);
        cache
            .set_with_strategy(key, value, CacheStrategy::ShortTerm)
            .await
            .unwrap();
    }

    // Invalidate all
    for key in &keys {
        cache.invalidate(key).await.unwrap();
    }

    // Wait for propagation
    sleep(Duration::from_millis(100)).await;

    // All should be gone
    for key in &keys {
        let cached = cache.get(key).await.unwrap();
        assert_eq!(cached, None);
    }
}
