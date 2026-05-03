use bytes::Bytes;
use multi_tier_cache::error::CacheError;
use multi_tier_cache::{CacheStrategy, CacheSystem};
use std::time::Duration;
use tokio::time::sleep;

mod common;

#[tokio::test]
async fn test_real_time_strategy_expiry() -> anyhow::Result<()> {
    // Setup
    let cache = CacheSystem::new().await?;
    let manager = cache.cache_manager();
    let key = common::test_key("realtime");
    let value = Bytes::from("{\"status\": \"live\"}");

    // 1. Set with RealTime strategy (10s ideally, but we test expiration logic)
    // We use Custom for faster test execution
    manager
        .set_with_strategy(
            &key,
            value.clone(),
            CacheStrategy::Custom(Duration::from_secs(1)),
        )
        .await?;

    // 2. Verify immediate retrieval
    let retrieved = manager.get(&key).await?;
    assert_eq!(retrieved, Some(value));

    // 3. Wait for expiration (1.1s)
    sleep(Duration::from_millis(1100)).await;

    // 4. Verify expiration
    let expired = manager.get(&key).await?;
    assert_eq!(expired, None, "Key should have expired");

    Ok(())
}

#[tokio::test]
async fn test_manual_l1_invalidation_check() -> anyhow::Result<()> {
    // This test verifies that we can manually invalidation works
    // Note: Cross-instance invalidation is tested in integration_invalidation.rs
    // This tests the local mechanism via direct removal.

    let cache = CacheSystem::new().await?;
    let manager = cache.cache_manager();
    let key = common::test_key("manual_inv");
    let value = Bytes::from("{\"data\": 123}");

    manager
        .set_with_strategy(&key, value.clone(), CacheStrategy::ShortTerm)
        .await?;

    // Ensure it's there
    assert!(manager.get(&key).await?.is_some());

    // Invalidate
    manager.invalidate(&key).await?;

    // Verify gone
    assert!(manager.get(&key).await?.is_none());

    Ok(())
}

#[tokio::test]
async fn test_get_or_compute_error_propagation() -> anyhow::Result<()> {
    let cache = CacheSystem::new().await?;
    let manager = cache.cache_manager();
    let key = common::test_key("compute_err");

    // Compute function that fails
    let result = manager
        .get_or_compute_with(&key, CacheStrategy::ShortTerm, || async {
            Err(CacheError::InternalError("Database failure".to_string()))
        })
        .await;

    assert!(result.is_err());
    assert_eq!(
        result
            .err()
            .unwrap_or_else(|| panic!("Should be error"))
            .to_string(),
        "Internal error: Database failure"
    );

    // Verify nothing cached
    assert!(manager.get(&key).await?.is_none());

    Ok(())
}
