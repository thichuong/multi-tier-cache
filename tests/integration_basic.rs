//! Basic integration tests for L1 and L2 cache operations
//!
//! These tests verify core functionality with real Redis instance.

mod common;

use common::*;
use multi_tier_cache::{CacheBackend, CacheStrategy};
use std::time::Duration;

/// Test basic cache set and get operations
#[tokio::test]
async fn test_basic_set_and_get() {
    let cache = setup_cache_system().await.expect("Failed to setup cache");
    let key = test_key("basic");
    let value = test_data::json_user(1);

    // Set value
    cache
        .cache_manager()
        .set_with_strategy(&key, value.clone(), CacheStrategy::ShortTerm)
        .await
        .expect("Failed to set value");

    // Get value
    let cached = cache
        .cache_manager()
        .get(&key)
        .await
        .expect("Failed to get value");

    assert_eq!(cached, Some(value));

    // Cleanup
    let _ = cache.l2_cache.as_ref().unwrap().remove(&key).await;
}

/// Test L1 hit path
#[tokio::test]
async fn test_l1_cache_hit() {
    let cache = setup_cache_system().await.unwrap();
    let key = test_key("l1_hit");
    let value = test_data::json_user(2);

    // Set value (populates L1)
    cache
        .cache_manager()
        .set_with_strategy(&key, value.clone(), CacheStrategy::MediumTerm)
        .await
        .unwrap();

    // First get - should hit L1
    let _ = cache.cache_manager().get(&key).await.unwrap();

    // Second get - should also hit L1
    let cached = cache.cache_manager().get(&key).await.unwrap();
    assert_eq!(cached, Some(value));

    // Verify L1 hits
    let stats = cache.cache_manager().get_stats();
    assert!(stats.l1_hits >= 1, "Expected at least 1 L1 hit");

    // Cleanup
    let _ = cache.l2_cache.as_ref().unwrap().remove(&key).await;
}

/// Test L2-to-L1 promotion
#[tokio::test]
async fn test_l2_to_l1_promotion() {
    let cache = setup_cache_system().await.unwrap();
    let key = test_key("l2_promote");
    let value = test_data::json_user(3);

    // Set directly in L2 (bypass L1)
    cache
        .l2_cache
        .as_ref()
        .unwrap()
        .set_with_ttl(&key, value.clone(), Duration::from_secs(300))
        .await
        .unwrap();

    // Get from cache manager - should promote to L1
    let cached = cache.cache_manager().get(&key).await.unwrap();
    assert_eq!(cached, Some(value.clone()));

    // Verify promotion occurred
    let stats = cache.cache_manager().get_stats();
    assert!(stats.promotions >= 1, "Expected at least 1 promotion");

    // Next get should hit L1
    let cached2 = cache.cache_manager().get(&key).await.unwrap();
    assert_eq!(cached2, Some(value));

    // Cleanup
    let _ = cache.l2_cache.as_ref().unwrap().remove(&key).await;
}

/// Test cache miss behavior
#[tokio::test]
async fn test_cache_miss() {
    let cache = setup_cache_system().await.unwrap();
    let key = test_key("miss");

    // Get non-existent key
    let cached = cache.cache_manager().get(&key).await.unwrap();
    assert_eq!(cached, None);

    // Verify miss was counted
    let stats = cache.cache_manager().get_stats();
    assert!(stats.misses >= 1, "Expected at least 1 miss");
}

/// Test compute-on-miss pattern
#[tokio::test]
async fn test_compute_on_miss() {
    let cache = setup_cache_system().await.unwrap();
    let key = test_key("compute");
    let expected_value = test_data::json_user(4);

    // Compute on miss
    let value = cache
        .cache_manager()
        .get_or_compute_with(&key, CacheStrategy::ShortTerm, || {
            let v = expected_value.clone();
            async move { Ok(v) }
        })
        .await
        .unwrap();

    assert_eq!(value, expected_value);

    // Verify it's now cached
    let cached = cache.cache_manager().get(&key).await.unwrap();
    assert_eq!(cached, Some(expected_value));

    // Cleanup
    let _ = cache.l2_cache.as_ref().unwrap().remove(&key).await;
}

/// Test type-safe caching
#[tokio::test]
async fn test_type_safe_caching() {
    let cache = setup_cache_system().await.unwrap();
    let key = test_key("typed");
    let expected_user = test_data::User::new(5);

    // Store and retrieve with type safety
    let user: test_data::User = cache
        .cache_manager()
        .get_or_compute_typed(&key, CacheStrategy::MediumTerm, || {
            let u = expected_user.clone();
            async move { Ok(u) }
        })
        .await
        .unwrap();

    assert_eq!(user, expected_user);

    // Verify it's cached
    let user2: test_data::User = cache
        .cache_manager()
        .get_or_compute_typed(&key, CacheStrategy::MediumTerm, || async {
            panic!("Should not compute again");
        })
        .await
        .unwrap();

    assert_eq!(user2, expected_user);

    // Cleanup
    let _ = cache.l2_cache.as_ref().unwrap().remove(&key).await;
}

/// Test TTL expiration
#[tokio::test]
async fn test_ttl_expiration() {
    let cache = setup_cache_system().await.unwrap();
    let key = test_key("ttl");
    let value = test_data::json_user(6);

    // Set with very short TTL
    cache
        .cache_manager()
        .set_with_strategy(
            &key,
            value.clone(),
            CacheStrategy::Custom(Duration::from_millis(100)),
        )
        .await
        .unwrap();

    // Immediate get should work
    let cached = cache.cache_manager().get(&key).await.unwrap();
    assert_eq!(cached, Some(value.clone()));

    // Wait for expiration
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Should be expired now
    let cached2 = cache.cache_manager().get(&key).await.unwrap();
    assert_eq!(cached2, None);

    // Cleanup
    let _ = cache.l2_cache.as_ref().unwrap().remove(&key).await;
}

/// Test cache statistics tracking
#[tokio::test]
async fn test_statistics_tracking() {
    let cache = setup_cache_system().await.unwrap();
    let key = test_key("stats");
    let value = test_data::json_user(7);

    // Initial stats
    let stats_before = cache.cache_manager().get_stats();

    // Perform operations
    cache
        .cache_manager()
        .set_with_strategy(&key, value, CacheStrategy::ShortTerm)
        .await
        .unwrap();

    let _ = cache.cache_manager().get(&key).await.unwrap(); // L1 hit
    let _ = cache
        .cache_manager()
        .get(&test_key("nonexistent"))
        .await
        .unwrap(); // Miss

    // Check stats increased
    let stats_after = cache.cache_manager().get_stats();
    assert!(stats_after.total_requests > stats_before.total_requests);
    assert!(
        stats_after.l1_hits > stats_before.l1_hits || stats_after.l2_hits > stats_before.l2_hits
    );
    assert!(stats_after.misses > stats_before.misses);

    // Cleanup
    let _ = cache.l2_cache.as_ref().unwrap().remove(&key).await;
}

/// Test health check functionality
#[tokio::test]
async fn test_health_check() {
    let cache = setup_cache_system().await.unwrap();

    let healthy = cache.health_check().await;
    assert!(healthy, "Cache system should be healthy");
}

/// Test different cache strategies
#[tokio::test]
async fn test_cache_strategies() {
    let cache = setup_cache_system().await.unwrap();

    let strategies = vec![
        ("realtime", CacheStrategy::RealTime),
        ("short", CacheStrategy::ShortTerm),
        ("medium", CacheStrategy::MediumTerm),
        ("long", CacheStrategy::LongTerm),
        ("custom", CacheStrategy::Custom(Duration::from_secs(60))),
    ];

    for (name, strategy) in strategies {
        let key = test_key(name);
        let value = test_data::json_user(8);

        cache
            .cache_manager()
            .set_with_strategy(&key, value.clone(), strategy)
            .await
            .expect(&format!("Failed to set with {} strategy", name));

        let cached = cache
            .cache_manager()
            .get(&key)
            .await
            .expect(&format!("Failed to get with {} strategy", name));

        assert_eq!(cached, Some(value));

        // Cleanup
        let _ = cache.l2_cache.as_ref().unwrap().remove(&key).await;
    }
}
