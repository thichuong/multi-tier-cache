//! Basic Usage Example
//!
//! Demonstrates simple cache operations: set, get, and health check.
//!
//! Run with: cargo run --example basic_usage

use multi_tier_cache::{CacheStrategy, CacheSystem};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("=== Multi-Tier Cache: Basic Usage ===\n");

    // 1. Initialize cache system (Default: L1 Moka + L2 Redis)
    let cache = CacheSystem::new().await?;
    println!();

    // 2. Health check
    if cache.health_check().await {
        println!("✅ Cache system is healthy\n");
    }

    // 3. Store data with different strategies
    let user_data = serde_json::json!({
        "id": 1,
        "name": "Alice",
        "email": "alice@example.com",
        "role": "admin"
    });

    println!("Storing user data with ShortTerm strategy (5 min TTL)...");
    cache
        .cache_manager()
        .set_with_strategy("user:1", user_data.clone(), CacheStrategy::ShortTerm)
        .await?;
    println!();

    // 4. Retrieve data
    println!("Retrieving user data...");
    if let Some(cached_user) = cache.cache_manager().get("user:1").await? {
        println!("✅ Retrieved from cache: {}", cached_user);
    }
    println!();

    // 5. Store API response with RealTime strategy
    let api_response = serde_json::json!({
        "timestamp": "2025-01-01T00:00:00Z",
        "temperature": 25.5,
        "humidity": 60
    });

    println!("Storing API response with RealTime strategy (10 sec TTL)...");
    cache
        .cache_manager()
        .set_with_strategy("sensor:temp", api_response, CacheStrategy::RealTime)
        .await?;
    println!();

    // 6. Get cache statistics
    let stats = cache.cache_manager().get_stats();
    println!("=== Cache Statistics ===");
    println!("Total requests: {}", stats.total_requests);
    println!("L1 hits: {}", stats.l1_hits);
    println!("L2 hits: {}", stats.l2_hits);
    println!("Misses: {}", stats.misses);
    println!("Hit rate: {:.2}%", stats.hit_rate);
    println!("L1 hit rate: {:.2}%", stats.l1_hit_rate);
    println!("Promotions (L2→L1): {}", stats.promotions);

    Ok(())
}
