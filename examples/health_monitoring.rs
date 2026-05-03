//! Health Monitoring Example
//!
//! Demonstrates health checks and monitoring of cache tiers.
//!
//! Run with: cargo run --example `health_monitoring`

use multi_tier_cache::CacheSystem;
use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("=== Multi-Tier Cache: Health Monitoring ===\n");

    let cache = CacheSystem::new().await?;
    println!();

    // Continuous health monitoring
    println!("Starting health monitoring (10 checks)...\n");

    for i in 1..=10 {
        println!("Health Check #{i}");

        // Check overall cache system health
        let is_healthy = cache.health_check().await;

        if is_healthy {
            println!("   ✅ Status: HEALTHY");
        } else {
            println!("   ⚠️  Status: DEGRADED (L2 may be down, L1 still operational)");
        }

        // Get detailed statistics
        let stats = cache.cache_manager().get_stats();
        println!("   📊 Statistics:");
        println!("      - Total requests: {}", stats.total_requests);
        println!("      - Hit rate: {:.2}%", stats.hit_rate);
        println!("      - L1 hit rate: {:.2}%", stats.l1_hit_rate);
        println!("      - Promotions: {}", stats.promotions);

        // Performance indicator
        if stats.l1_hit_rate > 80.0 {
            println!("   🚀 Performance: EXCELLENT (L1 hit rate > 80%)");
        } else if stats.l1_hit_rate > 50.0 {
            println!("   ⚡ Performance: GOOD (L1 hit rate > 50%)");
        } else {
            println!("   💡 Performance: CONSIDER OPTIMIZING (L1 hit rate < 50%)");
        }

        println!();

        // Sleep between checks
        if i < 10 {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    println!("=== Health Monitoring Tips ===");
    println!("1. Monitor L1 hit rate - should be > 80% for optimal performance");
    println!("2. Watch for promotions - indicates L2 is being used effectively");
    println!("3. Check in-flight requests - high numbers may indicate stampede");
    println!("4. L2 failure is tolerated - system continues with L1 only");

    Ok(())
}
