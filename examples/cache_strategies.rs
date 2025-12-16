//! Cache Strategies Example
//!
//! Demonstrates different caching strategies for various use cases.
//!
//! Run with: cargo run --example `cache_strategies`

use multi_tier_cache::{CacheStrategy, CacheSystem};
use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("=== Multi-Tier Cache: Caching Strategies ===\n");

    let cache = CacheSystem::new().await?;
    println!();

    // 1. RealTime strategy (10 seconds) - for rapidly changing data
    println!("1. RealTime Strategy (10s TTL) - Fast-changing data");
    let live_data = serde_json::json!({
        "price": 42000.50,
        "volume": 1_000_000,
        "last_updated": "2025-01-01T00:00:00Z"
    });
    cache
        .cache_manager()
        .set_with_strategy("live_price", &live_data, CacheStrategy::RealTime)
        .await?;
    println!("   ✅ Stored live price data (expires in 10 seconds)\n");

    // 2. ShortTerm strategy (5 minutes) - for frequently accessed data
    println!("2. ShortTerm Strategy (5 min TTL) - Frequently accessed");
    let user_session = serde_json::json!({
        "user_id": 123,
        "session_token": "abc123",
        "last_activity": "2025-01-01T00:00:00Z"
    });
    cache
        .cache_manager()
        .set_with_strategy("session:123", &user_session, CacheStrategy::ShortTerm)
        .await?;
    println!("   ✅ Stored user session (expires in 5 minutes)\n");

    // 3. MediumTerm strategy (1 hour) - for moderately stable data
    println!("3. MediumTerm Strategy (1 hour TTL) - Moderately stable");
    let product_catalog = serde_json::json!({
        "category": "electronics",
        "items": ["laptop", "phone", "tablet"],
        "updated": "2025-01-01"
    });
    cache
        .cache_manager()
        .set_with_strategy(
            "catalog:electronics",
            &product_catalog,
            CacheStrategy::MediumTerm,
        )
        .await?;
    println!("   ✅ Stored product catalog (expires in 1 hour)\n");

    // 4. LongTerm strategy (3 hours) - for stable data
    println!("4. LongTerm Strategy (3 hours TTL) - Stable data");
    let config_data = serde_json::json!({
        "api_version": "v1",
        "features": ["caching", "streaming", "analytics"],
        "maintenance_window": "Sunday 02:00 AM"
    });
    cache
        .cache_manager()
        .set_with_strategy("config:app", &config_data, CacheStrategy::MediumTerm)
        .await?;
    println!("   ✅ Stored application config (expires in 3 hours)\n");

    // 5. Custom strategy - for specific requirements
    println!("5. Custom Strategy (30s TTL) - Custom requirement");
    let custom_data = serde_json::json!({
        "metric": "cpu_usage",
        "value": 45.2,
        "threshold": 80.0
    });
    cache
        .cache_manager()
        .set_with_strategy(
            "metrics:cpu",
            &custom_data,
            CacheStrategy::Custom(Duration::from_secs(30)),
        )
        .await?;
    println!("   ✅ Stored custom metrics (expires in 30 seconds)\n");

    // 6. Default strategy (5 minutes)
    println!("6. Default Strategy (5 min TTL) - Fallback option");
    let generic_data = serde_json::json!({"key": "value"});
    cache
        .cache_manager()
        .set_with_strategy("generic_key", &generic_data, CacheStrategy::Default)
        .await?;
    println!("   ✅ Stored generic data (expires in 5 minutes)\n");

    // Retrieve all cached values
    println!("=== Verifying Cached Data ===\n");
    let keys = vec![
        "live_price",
        "session:123",
        "catalog:electronics",
        "app_config",
        "metrics:cpu",
        "generic_key",
    ];

    for key in keys {
        if let Some(value) = cache.cache_manager().get::<serde_json::Value>(key).await? {
            println!("✅ {key}: {value}");
        }
    }

    // Show cache statistics
    println!("\n=== Cache Statistics ===");
    let stats = cache.cache_manager().get_stats();
    println!("Total requests: {}", stats.total_requests);
    println!("Hit rate: {:.2}%", stats.hit_rate);

    println!("\n=== Strategy Recommendations ===");
    println!("RealTime (10s):    Stock prices, live scores, real-time sensor data");
    println!("ShortTerm (5min):  User sessions, API responses, search results");
    println!("MediumTerm (1hr):  Product catalogs, user profiles, configuration");
    println!("LongTerm (3hr):    Static content, reference data, app settings");
    println!("Custom:            Any specific TTL requirement");

    Ok(())
}
