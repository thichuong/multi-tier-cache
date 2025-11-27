//! Integration tests for stampede protection
//!
//! Tests concurrent access patterns and request coalescing

mod common;

use common::*;
use multi_tier_cache::{CacheBackend, CacheStrategy};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use tokio::task::JoinSet;

/// Test stampede protection with concurrent requests
#[tokio::test]
async fn test_concurrent_cache_miss() {
    let cache = Arc::new(
        setup_cache_system()
            .await
            .unwrap_or_else(|_| panic!("Failed to setup cache system")),
    );
    let key = test_key("stampede");
    let compute_count = Arc::new(AtomicU32::new(0));

    // Spawn 100 concurrent requests for same key
    let mut tasks = JoinSet::new();
    for _ in 0..100 {
        let cache_clone = Arc::clone(&cache);
        let key_clone = key.clone();
        let counter_clone = Arc::clone(&compute_count);

        tasks.spawn(async move {
            cache_clone
                .cache_manager()
                .get_or_compute_with(&key_clone, CacheStrategy::ShortTerm, || {
                    counter_clone.fetch_add(1, Ordering::SeqCst);
                    async move { Ok(test_data::json_user(1)) }
                })
                .await
        });
    }

    // Wait for all tasks
    while let Some(result) = tasks.join_next().await {
        result
            .unwrap_or_else(|_| panic!("Task panicked"))
            .unwrap_or_else(|_| panic!("Compute failed"));
    }

    // Stampede protection: only ONE compute should have happened
    let compute_calls = compute_count.load(Ordering::SeqCst);
    assert_eq!(
        compute_calls, 1,
        "Expected exactly 1 compute call, got {compute_calls}",
    );

    // Cleanup
    let _ = cache
        .l2_cache
        .as_ref()
        .unwrap_or_else(|| panic!("L2 cache missing"))
        .remove(&key)
        .await;
}

/// Test concurrent reads (all should be fast)
#[tokio::test]
async fn test_concurrent_cache_hits() {
    let cache = Arc::new(
        setup_cache_system()
            .await
            .unwrap_or_else(|_| panic!("Failed to setup cache system")),
    );
    let key = test_key("concurrent_hits");
    let value = test_data::json_user(2);

    // Pre-populate cache
    cache
        .cache_manager()
        .set_with_strategy(&key, value.clone(), CacheStrategy::MediumTerm)
        .await
        .unwrap_or_else(|_| panic!("Failed to set cache"));

    // Spawn 50 concurrent reads
    let mut tasks = JoinSet::new();
    for _ in 0..50 {
        let cache_clone = Arc::clone(&cache);
        let key_clone = key.clone();
        let expected = value.clone();

        tasks.spawn(async move {
            let cached = cache_clone
                .cache_manager()
                .get(&key_clone)
                .await
                .unwrap_or_else(|_| panic!("Failed to get cache"));
            assert_eq!(cached, Some(expected));
        });
    }

    // Wait for all
    while let Some(result) = tasks.join_next().await {
        result.unwrap_or_else(|_| panic!("Task failed"));
    }

    // Cleanup
    let _ = cache
        .l2_cache
        .as_ref()
        .unwrap_or_else(|| panic!("L2 cache missing"))
        .remove(&key)
        .await;
}

/// Test that stampede protection reduces latency
#[tokio::test]
async fn test_stampede_latency_reduction() {
    let cache = Arc::new(
        setup_cache_system()
            .await
            .unwrap_or_else(|_| panic!("Failed to setup cache system")),
    );
    let key = test_key("latency");

    // Without cache: simulate slow operation
    let start = std::time::Instant::now();
    let _ = cache
        .cache_manager()
        .get_or_compute_with(&key, CacheStrategy::ShortTerm, || async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            Ok(test_data::json_user(3))
        })
        .await
        .unwrap_or_else(|_| panic!("Failed to get/compute"));
    let first_duration = start.elapsed();

    // With cache: should be fast
    let start = std::time::Instant::now();
    let _ = cache
        .cache_manager()
        .get(&key)
        .await
        .unwrap_or_else(|_| panic!("Failed to get cache"));
    let cached_duration = start.elapsed();

    // Cached access should be significantly faster
    assert!(
        cached_duration < first_duration / 10,
        "Cached access should be at least 10x faster"
    );

    // Cleanup
    let _ = cache
        .l2_cache
        .as_ref()
        .unwrap_or_else(|| panic!("L2 cache missing"))
        .remove(&key)
        .await;
}
