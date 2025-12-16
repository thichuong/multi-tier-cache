//! Advanced Usage Example
//!
//! Demonstrates advanced features like L2-to-L1 promotion and custom workflows.
//!
//! Run with: cargo run --example `advanced_usage`

use multi_tier_cache::{CacheStrategy, CacheSystem};
use std::time::Duration;

async fn fetch_from_database(id: u32) -> anyhow::Result<serde_json::Value> {
    println!("   📦 Fetching from database (expensive operation)...");
    tokio::time::sleep(Duration::from_millis(200)).await;
    Ok(serde_json::json!({
        "product_id": id,
        "name": format!("Product {id}"),
        "price": 100 + id
    }))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("=== Multi-Tier Cache: Advanced Usage ===\n");

    let cache = CacheSystem::new().await?;
    println!();

    // Scenario 1: L2-to-L1 Promotion
    // Note: In multi-tier systems (L1+L2+L3...), promotion happens from any lower tier
    // to all upper tiers (e.g., L3 -> L2 and L1).
    println!("=== Scenario 1: L2-to-L1 Cache Promotion ===\n");

    // Store data in L2 cache only (by first clearing L1)
    let data = serde_json::json!({"message": "This data starts in L2"});
    cache
        .cache_manager()
        .set_with_strategy("promotion_test", &data, CacheStrategy::MediumTerm)
        .await?;
    println!("✅ Data stored in both L1 and L2\n");

    // First access - hits L1
    println!("First access:");
    let result1 = cache
        .cache_manager()
        .get::<serde_json::Value>("promotion_test")
        .await?;
    println!("   Retrieved: {:?}\n", result1.is_some());

    // Wait for L1 to expire (simulating L1 eviction)
    println!("Waiting 6 seconds for L1 TTL to expire...");
    tokio::time::sleep(Duration::from_secs(6)).await;

    // Second access - should hit L2 and promote to L1
    println!("Second access (after L1 expiration):");
    let result2 = cache
        .cache_manager()
        .get::<serde_json::Value>("promotion_test")
        .await?;
    println!("   Retrieved: {:?}", result2.is_some());
    println!("   (Data promoted from L2 back to L1)\n");

    // Third access - should hit L1 again
    println!("Third access (now in L1):");
    let result3 = cache
        .cache_manager()
        .get::<serde_json::Value>("promotion_test")
        .await?;
    println!("   Retrieved: {:?}\n", result3.is_some());

    // Scenario 2: get_or_compute_with pattern
    println!("=== Scenario 2: Compute-on-Miss Pattern ===\n");

    // First call - cache miss, will compute
    println!("First call - cache miss:");
    let product1 = cache
        .cache_manager()
        .get_or_compute_with("product:42", CacheStrategy::MediumTerm, || {
            fetch_from_database(42)
        })
        .await?;
    println!("   Result: {product1}\n");

    // Second call - cache hit, no computation
    println!("Second call - cache hit:");
    let product2 = cache
        .cache_manager()
        .get_or_compute_with("product:42", CacheStrategy::MediumTerm, || {
            fetch_from_database(42)
        })
        .await?;
    println!("   Result: {product2} (from cache, no DB call)\n");

    // Scenario 3: Concurrent cache operations
    println!("=== Scenario 3: Concurrent Operations ===\n");

    let mut handles = vec![];
    for i in 1..=5 {
        let cache_clone = cache.clone();
        let handle = tokio::spawn(async move {
            let data = serde_json::json!({
                "worker_id": i,
                "data": format!("Concurrent data from worker {}", i)
            });
            cache_clone
                .cache_manager()
                .set_with_strategy(&format!("concurrent:{i}"), &data, CacheStrategy::ShortTerm)
                .await
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.await??;
    }
    println!("✅ 5 concurrent cache operations completed\n");

    // Verify all concurrent writes
    for i in 1..=5 {
        if let Some(value) = cache
            .cache_manager()
            .get::<serde_json::Value>(&format!("concurrent:{i}"))
            .await?
        {
            println!("   concurrent:{i} = {value}");
        }
    }

    // Final statistics
    println!("\n=== Final Cache Statistics ===");
    let stats = cache.cache_manager().get_stats();
    println!("Total requests: {}", stats.total_requests);
    println!("L1 hits: {} ({:.2}%)", stats.l1_hits, stats.l1_hit_rate);
    println!("L2 hits: {}", stats.l2_hits);
    println!("Misses: {}", stats.misses);
    println!("Overall hit rate: {:.2}%", stats.hit_rate);
    println!("L2→L1 promotions: {}", stats.promotions);
    println!("In-flight requests: {}", stats.in_flight_requests);

    Ok(())
}
