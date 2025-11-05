//! Integration tests for multi-tier cache architecture (v0.5.0+)

use multi_tier_cache::{CacheSystemBuilder, CacheStrategy, TierConfig, L2Cache};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

mod common;

/// Test basic multi-tier get/set operations
#[tokio::test]
async fn test_multi_tier_basic_operations() {
    // Create 3-tier cache: L1 + L2 + L3 (all using L2Cache for testing)
    // Note: In production, L1 would be Moka-based, but for multi-tier mode,
    // all backends must implement L2CacheBackend for TTL support
    let l1 = Arc::new(L2Cache::new().await.unwrap());
    let l2 = Arc::new(L2Cache::new().await.unwrap());
    let l3 = Arc::new(L2Cache::new().await.unwrap());

    let cache = CacheSystemBuilder::new()
        .with_tier(l1, TierConfig::as_l1())
        .with_tier(l2, TierConfig::as_l2())
        .with_tier(l3, TierConfig::as_l3())
        .build()
        .await
        .unwrap();

    let manager = cache.cache_manager();

    // Test set_with_strategy - should store in all tiers
    let test_data = json!({"user": "alice", "id": 123});
    manager
        .set_with_strategy("test:multi:1", test_data.clone(), CacheStrategy::ShortTerm)
        .await
        .unwrap();

    // Test get - should hit L1
    let result = manager.get("test:multi:1").await.unwrap();
    assert_eq!(result, Some(test_data.clone()));

    // Verify tier stats
    if let Some(tier_stats) = manager.get_tier_stats() {
        println!("Multi-tier stats:");
        for stats in &tier_stats {
            println!("  L{}: {} hits ({})",
                     stats.tier_level,
                     stats.hit_count(),
                     stats.backend_name);
        }
        assert_eq!(tier_stats.len(), 3, "Should have 3 tiers");
    } else {
        panic!("Expected tier stats for multi-tier mode");
    }

    println!("✅ Multi-tier basic operations test passed");
}

/// Test multi-tier statistics tracking
#[tokio::test]
async fn test_multi_tier_stats() {
    let l1 = Arc::new(L2Cache::new().await.unwrap());
    let l2 = Arc::new(L2Cache::new().await.unwrap());
    let l3 = Arc::new(L2Cache::new().await.unwrap());

    let cache = CacheSystemBuilder::new()
        .with_tier(l1.clone(), TierConfig::as_l1())
        .with_tier(l2.clone(), TierConfig::as_l2())
        .with_tier(l3.clone(), TierConfig::as_l3())
        .build()
        .await
        .unwrap();

    let manager = cache.cache_manager();

    // Store some data
    let test_data = json!({"stats": "test"});
    manager
        .set_with_strategy("test:stats:1", test_data.clone(), CacheStrategy::ShortTerm)
        .await
        .unwrap();

    // Retrieve data multiple times
    for _ in 0..5 {
        let _result = manager.get("test:stats:1").await.unwrap();
    }

    // Verify tier-specific stats
    if let Some(tier_stats) = manager.get_tier_stats() {
        assert_eq!(tier_stats.len(), 3, "Should have 3 tiers");

        // L1 should have most hits
        let l1_stats = tier_stats.iter().find(|s| s.tier_level == 1).unwrap();
        assert!(l1_stats.hit_count() >= 4, "L1 should have at least 4 hits from repeated gets");

        println!("Tier statistics:");
        for stats in &tier_stats {
            println!("  L{}: {} hits", stats.tier_level, stats.hit_count());
        }
    }

    // Verify overall stats
    let stats = manager.get_stats();
    assert!(stats.total_requests >= 5, "Should track all requests");
    assert!(stats.l1_hits >= 4, "Should have L1 hits");

    println!("✅ Multi-tier statistics test passed");
}

/// Test backward compatibility - legacy 2-tier mode should still work
#[tokio::test]
async fn test_backward_compatibility_legacy_mode() {
    // Use old-style constructor (no tiers)
    let cache = CacheSystemBuilder::new()
        .build()
        .await
        .unwrap();

    let manager = cache.cache_manager();

    // Standard operations should work
    let test_data = json!({"legacy": "mode"});
    manager
        .set_with_strategy("test:legacy:1", test_data.clone(), CacheStrategy::ShortTerm)
        .await
        .unwrap();

    let result = manager.get("test:legacy:1").await.unwrap();
    assert_eq!(result, Some(test_data));

    // Tier stats should be None for legacy mode
    assert!(manager.get_tier_stats().is_none(), "Legacy mode should not have tier stats");

    // Regular stats should work
    let stats = manager.get_stats();
    assert!(stats.total_requests > 0, "Should have request stats");

    println!("✅ Backward compatibility test passed");
}

/// Test TTL scaling across tiers
#[tokio::test]
async fn test_multi_tier_ttl_scaling() {
    let l1 = Arc::new(L2Cache::new().await.unwrap());
    let l2 = Arc::new(L2Cache::new().await.unwrap());
    let l3 = Arc::new(L2Cache::new().await.unwrap());

    let cache = CacheSystemBuilder::new()
        .with_tier(l1, TierConfig::as_l1())
        .with_tier(l2, TierConfig::as_l2())
        .with_tier(
            l3,
            TierConfig::as_l3() // L3 has 2x TTL multiplier
        )
        .build()
        .await
        .unwrap();

    let manager = cache.cache_manager();

    // Set with 10 second TTL
    let test_data = json!({"ttl": "test"});
    manager
        .set_with_strategy("test:ttl:1", test_data, CacheStrategy::Custom(Duration::from_secs(10)))
        .await
        .unwrap();

    // L1 and L2 should have 10s TTL, L3 should have 20s TTL (2x multiplier)
    // We can't directly verify TTL from outside, but the operation should succeed

    let result = manager.get("test:ttl:1").await.unwrap();
    assert!(result.is_some(), "Should retrieve data with scaled TTL");

    println!("✅ Multi-tier TTL scaling test passed");
}

/// Test multi-tier cache miss
#[tokio::test]
async fn test_multi_tier_cache_miss() {
    let l1 = Arc::new(L2Cache::new().await.unwrap());
    let l2 = Arc::new(L2Cache::new().await.unwrap());

    let cache = CacheSystemBuilder::new()
        .with_tier(l1, TierConfig::as_l1())
        .with_tier(l2, TierConfig::as_l2())
        .build()
        .await
        .unwrap();

    let manager = cache.cache_manager();

    // Try to get non-existent key
    let result = manager.get("test:miss:nonexistent").await.unwrap();
    assert!(result.is_none(), "Should return None for cache miss");

    // Verify miss count
    let stats = manager.get_stats();
    assert!(stats.misses > 0, "Should track cache misses");

    println!("✅ Multi-tier cache miss test passed");
}

/// Test convenience methods: with_l3() and with_l4()
#[tokio::test]
async fn test_convenience_methods() {
    let l1_backend = Arc::new(L2Cache::new().await.unwrap());
    let l2_backend = Arc::new(L2Cache::new().await.unwrap());
    let l3_backend = Arc::new(L2Cache::new().await.unwrap());
    let l4_backend = Arc::new(L2Cache::new().await.unwrap());

    // Test with_l3() and with_l4() convenience methods
    let cache = CacheSystemBuilder::new()
        .with_tier(l1_backend, TierConfig::as_l1())
        .with_tier(l2_backend, TierConfig::as_l2())
        .with_l3(l3_backend)
        .with_l4(l4_backend)
        .build()
        .await
        .unwrap();

    let manager = cache.cache_manager();

    // Verify tier stats
    if let Some(tier_stats) = manager.get_tier_stats() {
        // Should have L1 + L2 + L3 + L4 = 4 tiers
        assert_eq!(tier_stats.len(), 4, "Should have 4 tiers");

        // Check that all tiers are present
        let has_l1 = tier_stats.iter().any(|s| s.tier_level == 1);
        let has_l2 = tier_stats.iter().any(|s| s.tier_level == 2);
        let has_l3 = tier_stats.iter().any(|s| s.tier_level == 3);
        let has_l4 = tier_stats.iter().any(|s| s.tier_level == 4);

        assert!(has_l1, "Should have L1 tier");
        assert!(has_l2, "Should have L2 tier");
        assert!(has_l3, "Should have L3 tier");
        assert!(has_l4, "Should have L4 tier");
    }

    println!("✅ Convenience methods test passed");
}
