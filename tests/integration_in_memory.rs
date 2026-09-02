//! Pure In-Memory Integration Tests
//!
//! These tests verify the entire multi-tier caching pipeline, stampede protection,
//! TTL scaling, and statistics tracking using pure in-memory backends (DashMapCache).
//! They run completely isolated without requiring an external Redis or network service.

use bytes::Bytes;
use multi_tier_cache::backends::DashMapCache;
use multi_tier_cache::error::CacheResult;
use multi_tier_cache::{
    CacheBackend, CacheManager, CacheStrategy, CacheSystemBuilder, L2CacheBackend, TierConfig,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;
use tokio::task::JoinSet;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct TestUser {
    id: u64,
    name: String,
    role: String,
}

/// Helper to construct a 2-tier in-memory CacheManager
fn setup_in_memory_2tier() -> CacheResult<Arc<CacheManager>> {
    let l1 = Arc::new(DashMapCache::new_with_capacity(100));
    let l2 = Arc::new(DashMapCache::new_with_capacity(500));

    let tiers = vec![
        multi_tier_cache::CacheTier::new(l1 as Arc<dyn L2CacheBackend>, 1, false, 1, 1.0),
        multi_tier_cache::CacheTier::new(l2 as Arc<dyn L2CacheBackend>, 2, true, 1, 2.0),
    ];

    let manager = CacheManager::new_with_tiers(tiers, None)?;
    Ok(Arc::new(manager))
}

#[tokio::test]
async fn test_in_memory_basic_operations() -> CacheResult<()> {
    let manager = setup_in_memory_2tier()?;
    let key = "user:profile:100";
    let data = Bytes::from("{\"name\": \"Alice\", \"active\": true}");

    // Set value across all tiers
    manager
        .set_with_strategy(key, data.clone(), CacheStrategy::ShortTerm)
        .await?;

    // First read should hit L1
    let hit1 = manager.get(key).await?;
    assert_eq!(hit1, Some(data.clone()));

    let stats = manager.get_stats();
    assert_eq!(stats.l1_hits, 1);
    assert_eq!(stats.misses, 0);

    Ok(())
}

#[tokio::test]
async fn test_in_memory_l2_promotion() -> CacheResult<()> {
    let l1 = Arc::new(DashMapCache::new());
    let l2 = Arc::new(DashMapCache::new());

    // Pre-populate only L2
    let key = "promoted:key";
    let value = Bytes::from("l2_only_value");
    l2.set_with_ttl(key, value.clone(), Duration::from_secs(60))
        .await?;

    let tiers = vec![
        multi_tier_cache::CacheTier::new(l1.clone() as Arc<dyn L2CacheBackend>, 1, false, 1, 1.0),
        multi_tier_cache::CacheTier::new(l2.clone() as Arc<dyn L2CacheBackend>, 2, true, 1, 1.0),
    ];
    let manager = CacheManager::new_with_tiers(tiers, None)?;

    // L1 should be empty initially
    assert_eq!(l1.get(key).await, None);

    // Reading through manager should hit L2 and promote to L1 (promotion_frequency = 1)
    let fetched = manager.get(key).await?;
    assert_eq!(fetched, Some(value.clone()));

    // Verify L1 is now populated due to promotion
    assert_eq!(l1.get(key).await, Some(value));

    let stats = manager.get_stats();
    assert_eq!(stats.l2_hits, 1);
    assert!(stats.promotions >= 1);

    Ok(())
}

#[tokio::test]
async fn test_in_memory_stampede_protection() -> CacheResult<()> {
    let manager = setup_in_memory_2tier()?;
    let key = "stampede:shared_key";
    let compute_counter = Arc::new(AtomicU32::new(0));

    let mut set = JoinSet::new();

    // 50 concurrent requests for the same non-cached key
    for _ in 0..50 {
        let mgr = Arc::clone(&manager);
        let counter = Arc::clone(&compute_counter);

        set.spawn(async move {
            mgr.get_or_compute_with(key, CacheStrategy::ShortTerm, || {
                counter.fetch_add(1, Ordering::SeqCst);
                async move {
                    // Simulate non-trivial work
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    Ok(Bytes::from("computed_result"))
                }
            })
            .await
        });
    }

    while let Some(res) = set.join_next().await {
        let bytes = res.expect("Task join failed")?;
        assert_eq!(bytes, Bytes::from("computed_result"));
    }

    // Coalescing must ensure compute function was called exactly ONCE
    let calls = compute_counter.load(Ordering::SeqCst);
    assert_eq!(calls, 1, "Expected exactly 1 compute call, but got {calls}");

    // Subsequent get should hit L1
    assert_eq!(manager.get(key).await?, Some(Bytes::from("computed_result")));

    Ok(())
}

#[tokio::test]
async fn test_in_memory_typed_compute_on_miss() -> CacheResult<()> {
    let manager = setup_in_memory_2tier()?;
    let key = "typed:user:42";

    let expected_user = TestUser {
        id: 42,
        name: "Bob".to_string(),
        role: "admin".to_string(),
    };

    let user_clone = expected_user.clone();
    let result: TestUser = manager
        .get_or_compute_typed(key, CacheStrategy::ShortTerm, || async move {
            Ok(user_clone)
        })
        .await?;

    assert_eq!(result, expected_user);

    // Get typed directly from cache
    let cached_user: Option<TestUser> = manager.get_typed(key).await?;
    assert_eq!(cached_user, Some(expected_user));

    Ok(())
}

#[tokio::test]
async fn test_in_memory_ttl_expiration() -> CacheResult<()> {
    let manager = setup_in_memory_2tier()?;
    let key = "expiring:key";
    let data = Bytes::from("short_lived_data");

    // Base TTL = 40ms. In setup_in_memory_2tier: L1 scale = 1.0 (40ms), L2 scale = 2.0 (80ms).
    manager
        .set_with_strategy(
            key,
            data.clone(),
            CacheStrategy::Custom(Duration::from_millis(40)),
        )
        .await?;

    // Immediate read should succeed
    assert_eq!(manager.get(key).await?, Some(data.clone()));

    // At 50ms: L1 (40ms) expired, but L2 (80ms) still holds the data
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        manager.get(key).await?,
        Some(data),
        "L2 should still hold data due to 2.0x TTL scaling"
    );

    // Wait until 100ms: now both L1 and L2 have expired
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert_eq!(
        manager.get(key).await?,
        None,
        "All tiers should now be expired"
    );

    Ok(())
}

#[tokio::test]
async fn test_in_memory_builder_multi_tier() -> CacheResult<()> {
    let l1 = Arc::new(DashMapCache::new());
    let l2 = Arc::new(DashMapCache::new());
    let l3 = Arc::new(DashMapCache::new());

    let cache_system = CacheSystemBuilder::new()
        .with_tier(l1 as Arc<dyn L2CacheBackend>, TierConfig::as_l1())
        .with_tier(l2 as Arc<dyn L2CacheBackend>, TierConfig::as_l2())
        .with_l3(l3 as Arc<dyn L2CacheBackend>)
        .build()
        .await?;

    let manager = cache_system.cache_manager();
    manager
        .set_with_strategy("3tier:key", Bytes::from("val"), CacheStrategy::ShortTerm)
        .await?;

    assert_eq!(manager.get("3tier:key").await?, Some(Bytes::from("val")));

    let tier_stats = manager.get_tier_stats();
    assert_eq!(tier_stats.len(), 3);
    assert_eq!(tier_stats[0].tier_level, 1);
    assert_eq!(tier_stats[1].tier_level, 2);
    assert_eq!(tier_stats[2].tier_level, 3);

    Ok(())
}
