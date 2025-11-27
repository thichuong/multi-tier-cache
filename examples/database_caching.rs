//! Type-Safe Database Caching Example
//!
//! This example demonstrates how to use `get_or_compute_typed()` for automatic
//! database query caching with type safety and zero boilerplate.
//!
//! Run with: `cargo run --example database_caching`

use multi_tier_cache::{CacheStrategy, CacheSystem};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Mock user struct representing a database row
#[derive(Debug, Clone, Serialize, Deserialize)]
struct User {
    id: i64,
    name: String,
    email: String,
    created_at: i64,
}

/// Mock product struct
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Product {
    id: i64,
    title: String,
    price: f64,
    stock: i32,
}

/// Simulate database query (in real world, use sqlx)
async fn fetch_user_from_db(user_id: i64) -> anyhow::Result<User> {
    println!("  🗄️  Simulating database query for user {}", user_id);
    tokio::time::sleep(Duration::from_millis(100)).await; // Simulate DB latency

    Ok(User {
        id: user_id,
        name: format!("User {}", user_id),
        email: format!("user{}@example.com", user_id),
        created_at: 1704326400, // 2024-01-04
    })
}

/// Simulate product query
async fn fetch_product_from_db(product_id: i64) -> anyhow::Result<Product> {
    println!("  🗄️  Simulating database query for product {}", product_id);
    tokio::time::sleep(Duration::from_millis(150)).await;

    Ok(Product {
        id: product_id,
        title: format!("Product #{}", product_id),
        price: 99.99 + (product_id as f64),
        stock: (product_id * 10) as i32,
    })
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("🎯 Type-Safe Database Caching Example\n");
    println!("This demonstrates automatic caching with type safety for database queries.\n");

    // Initialize cache system
    println!("📦 Initializing cache system...");
    let cache = CacheSystem::new().await?;
    println!("✅ Cache system ready!\n");

    // ========================================
    // Example 1: First Request (Cache Miss)
    // ========================================
    println!("📊 Example 1: First user request (will hit database)");
    println!("─────────────────────────────────────────────────────");

    let start = std::time::Instant::now();
    let user: User = cache
        .cache_manager()
        .get_or_compute_typed(
            "user:123",
            CacheStrategy::MediumTerm, // 1 hour TTL
            || async { fetch_user_from_db(123).await },
        )
        .await?;
    let elapsed = start.elapsed();

    println!("✅ Retrieved user: {:?}", user);
    println!(
        "⏱️  Time taken: {:?} (includes DB query + caching)\n",
        elapsed
    );

    // ========================================
    // Example 2: Second Request (Cache Hit)
    // ========================================
    println!("📊 Example 2: Second user request (will hit L1 cache)");
    println!("─────────────────────────────────────────────────────");

    let start = std::time::Instant::now();
    let user: User = cache
        .cache_manager()
        .get_or_compute_typed("user:123", CacheStrategy::MediumTerm, || async {
            fetch_user_from_db(123).await
        })
        .await?;
    let elapsed = start.elapsed();

    println!("✅ Retrieved user: {:?}", user);
    println!("⏱️  Time taken: {:?} (sub-millisecond from L1!)\n", elapsed);

    // ========================================
    // Example 3: Different Type (Product)
    // ========================================
    println!("📊 Example 3: Product query with different type");
    println!("────────────────────────────────────────────────");

    let product: Product = cache
        .cache_manager()
        .get_or_compute_typed(
            "product:456",
            CacheStrategy::LongTerm, // 3 hours TTL
            || async { fetch_product_from_db(456).await },
        )
        .await?;

    println!("✅ Retrieved product: {:?}\n", product);

    // ========================================
    // Example 4: Multiple Concurrent Requests
    // ========================================
    println!("📊 Example 4: Concurrent requests (stampede protection)");
    println!("────────────────────────────────────────────────────────");
    println!("Spawning 5 concurrent requests for user:999...");

    let cache_clone = cache.clone();
    let handles: Vec<_> = (0..5)
        .map(|i| {
            let cache = cache_clone.clone();
            tokio::spawn(async move {
                let start = std::time::Instant::now();
                let user: User = cache
                    .cache_manager()
                    .get_or_compute_typed("user:999", CacheStrategy::ShortTerm, || async {
                        fetch_user_from_db(999).await
                    })
                    .await
                    .unwrap();
                let elapsed = start.elapsed();
                println!("  🎯 Request {} completed in {:?}", i + 1, elapsed);
                user
            })
        })
        .collect();

    // Wait for all requests
    for handle in handles {
        handle.await?;
    }

    println!("\n💡 Notice: Only ONE database query was executed!");
    println!("   Cache stampede protection coalesced all 5 requests.\n");

    // ========================================
    // Statistics
    // ========================================
    println!("📈 Final Cache Statistics");
    println!("─────────────────────────");
    let stats = cache.cache_manager().get_stats();
    println!("Total requests: {}", stats.total_requests);
    println!("L1 hits: {}", stats.l1_hits);
    println!("L2 hits: {}", stats.l2_hits);
    println!("Cache misses: {}", stats.misses);
    println!("Hit rate: {:.2}%", stats.hit_rate);
    println!("Promotions: {}", stats.promotions);

    println!("\n✅ Example completed successfully!");
    println!(
        "
💡 Key Takeaways:
─────────────────
1. Type-safe: Compiler enforces correct types
2. Zero boilerplate: No manual serialize/deserialize
3. Automatic caching: L1+L2 storage handled for you
4. Stampede protection: Concurrent requests coalesced
5. Generic: Works with any Serialize + Deserialize type
"
    );

    Ok(())
}
