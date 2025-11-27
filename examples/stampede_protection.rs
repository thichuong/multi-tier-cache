//! Cache Stampede Protection Example
//!
//! Demonstrates how the library prevents cache stampede by coalescing
//! concurrent requests for the same cache key.
//!
//! Run with: cargo run --example `stampede_protection`

use multi_tier_cache::{CacheStrategy, CacheSystem};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Simulate expensive computation
async fn expensive_computation(id: u32) -> anyhow::Result<serde_json::Value> {
    println!("  💻 [Worker {id}] Starting expensive computation...");
    tokio::time::sleep(Duration::from_millis(500)).await;
    println!("  ✅ [Worker {id}] Computation complete");

    Ok(serde_json::json!({
        "result": "computed_value",
        "timestamp": 1_234_567_890,
        "worker_id": id
    }))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("=== Multi-Tier Cache: Stampede Protection Demo ===\n");

    // Initialize cache system
    let cache = Arc::new(CacheSystem::new().await?);
    println!();

    // Scenario: 10 concurrent requests for the same missing cache key
    println!("Scenario: 10 concurrent workers requesting same cache key\n");

    let start = Instant::now();
    let mut handles = vec![];

    for i in 1..=10 {
        let cache_clone = cache.clone();
        let handle = tokio::spawn(async move {
            let worker_start = Instant::now();

            // Try to get or compute
            let result = cache_clone
                .cache_manager()
                .get_or_compute_with("stampede_test_key", CacheStrategy::ShortTerm, || {
                    expensive_computation(i)
                })
                .await;

            let elapsed = worker_start.elapsed();
            println!("  [Worker {i}] Completed in {elapsed:?}");
            result
        });
        handles.push(handle);
    }

    // Wait for all workers
    for handle in handles {
        handle.await??;
    }

    let total_elapsed = start.elapsed();

    println!("\n=== Results ===");
    println!("Total time: {total_elapsed:?}");
    println!("Expected time WITHOUT stampede protection: ~5000ms (10 workers × 500ms)");
    println!("Expected time WITH stampede protection: ~500ms (1 computation shared by all)");

    if total_elapsed.as_millis() < 1000 {
        println!("✅ Stampede protection WORKING! Only 1 computation executed.");
    } else {
        println!("⚠️  Warning: Stampede protection may not be working optimally.");
    }

    // Show statistics
    let stats = cache.cache_manager().get_stats();
    println!("\n=== Cache Statistics ===");
    println!("Total requests: {}", stats.total_requests);
    println!("Cache hits: {}", stats.total_hits);
    println!(
        "In-flight requests (coalesced): {}",
        stats.in_flight_requests
    );

    Ok(())
}
