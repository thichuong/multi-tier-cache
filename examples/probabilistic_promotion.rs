#![allow(clippy::cast_precision_loss, clippy::cast_lossless)]
use multi_tier_cache::{
    CacheBackend, CacheSystemBuilder, L1Cache, L2Cache, L2CacheBackend, MokaCacheConfig, TierConfig,
};
use std::sync::Arc;
use tokio::time::Duration;
use tracing::{Level, info};
use tracing_subscriber::FmtSubscriber;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 1. Initialize tracing
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::DEBUG)
        .finish();
    let _ = tracing::subscriber::set_global_default(subscriber);

    info!("Starting Probabilistic Promotion Example");
    info!("------------------------------------------");

    // 2. Setup Multi-Tier Cache with N=10 (Probabilistic)
    // We want to promote from L2 to L1 only ~10% of the time
    let l1_concrete = Arc::new(L1Cache::new(MokaCacheConfig::default())?);
    let l2_concrete = Arc::new(L2Cache::new().await?);

    let cache = CacheSystemBuilder::new()
        .with_tier(
            Arc::clone(&l1_concrete) as Arc<dyn L2CacheBackend>,
            TierConfig::as_l1(),
        )
        .with_tier(
            Arc::clone(&l2_concrete) as Arc<dyn L2CacheBackend>,
            TierConfig::as_l2().with_promotion_frequency(10),
        )
        .build()
        .await?;

    let manager = cache.cache_manager();
    let key = "probabilistic_test_key";
    let value = bytes::Bytes::from("some_data");

    // 3. Populate L2 directly (bypass L1)
    l2_concrete
        .set_with_ttl(key, value.clone(), Duration::from_secs(300))
        .await?;
    info!("Key '{}' stored in L2. L1 is empty.", key);

    // 4. Access the key multiple times and observe promotions
    let iterations = 50;
    info!("Accessing key {} times with N=10...", iterations);

    let mut l1_first_hit_at = None;
    for i in 1..=iterations {
        let _ = manager.get(key).await?;

        let stats = manager.get_stats();
        let current_promotions = stats.promotions;

        // Check if it's now in L1
        if l1_concrete.get(key).await.is_some() && l1_first_hit_at.is_none() {
            l1_first_hit_at = Some(i);
            info!(
                "Iteration {}: FIRST Promotion occurred! Key is now in L1.",
                i
            );
        }

        if i % 10 == 0 {
            info!(
                "Progress: {}/{} requests. Current Promotions: {}",
                i, iterations, current_promotions
            );
        }
    }

    let final_stats = manager.get_stats();
    info!("------------------------------------------");
    info!("Final Results:");
    info!("Total Requests: {}", iterations);
    info!("Total Promotions: {}", final_stats.promotions);
    info!("First Promotion at: {:?}", l1_first_hit_at);
    info!(
        "Promotion Rate: {:.1}% (Expected ~10%)",
        (final_stats.promotions as f64 / f64::from(iterations)) * 100.0
    );
    info!("------------------------------------------");

    // Cleanup
    l2_concrete.remove(key).await?;
    info!("Example finished.");

    Ok(())
}
