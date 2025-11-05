//! Example: Built-in Cache Backends
//!
//! This example demonstrates how to use alternative built-in cache backends
//! instead of the default Moka (L1) and Redis (L2) implementations.
//!
//! Available backends:
//! - **L1 (In-Memory)**:
//!   - MokaCache (default) - High-performance with automatic eviction
//!   - DashMapCache - Simple concurrent HashMap-based cache
//!   - QuickCacheBackend - Lightweight, optimized for maximum performance (requires `backend-quickcache` feature)
//!
//! - **L2 (Distributed)**:
//!   - RedisCache (default) - Industry-standard with persistence
//!   - MemcachedCache - Lightweight distributed cache (requires `backend-memcached` feature)
//!
//! Run with:
//! ```bash
//! # Default backends (Moka + Redis)
//! cargo run --example builtin_backends
//!
//! # With all backends enabled
//! cargo run --example builtin_backends --all-features
//! ```

use multi_tier_cache::{CacheSystemBuilder, CacheStrategy, CacheBackend};
use anyhow::Result;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<()> {
    println!("=== Multi-Tier Cache: Built-in Backends Example ===\n");

    // Example 1: DashMapCache (L1)
    println!("📦 Example 1: DashMapCache (L1)");
    println!("─────────────────────────────────\n");

    demo_dashmap_backend().await?;

    // Example 2: MemcachedCache (L2) - requires feature flag
    #[cfg(feature = "backend-memcached")]
    {
        println!("\n📦 Example 2: MemcachedCache (L2)");
        println!("──────────────────────────────────\n");
        demo_memcached_backend().await?;
    }

    #[cfg(not(feature = "backend-memcached"))]
    {
        println!("\n📦 Example 2: MemcachedCache (L2)");
        println!("──────────────────────────────────");
        println!("⚠️  Skipped: Requires 'backend-memcached' feature");
        println!("   Run with: cargo run --example builtin_backends --features backend-memcached\n");
    }

    // Example 3: QuickCacheBackend (L1) - requires feature flag
    #[cfg(feature = "backend-quickcache")]
    {
        println!("\n📦 Example 3: QuickCacheBackend (L1)");
        println!("─────────────────────────────────────\n");
        demo_quickcache_backend().await?;
    }

    #[cfg(not(feature = "backend-quickcache"))]
    {
        println!("\n📦 Example 3: QuickCacheBackend (L1)");
        println!("─────────────────────────────────────");
        println!("⚠️  Skipped: Requires 'backend-quickcache' feature");
        println!("   Run with: cargo run --example builtin_backends --features backend-quickcache\n");
    }

    println!("\n✅ Built-in backends example completed!");

    Ok(())
}

/// Demonstrate DashMapCache as L1 backend
async fn demo_dashmap_backend() -> Result<()> {
    use multi_tier_cache::DashMapCache;

    println!("Using DashMapCache as L1 backend...");

    // Create DashMapCache
    let dashmap_l1 = Arc::new(DashMapCache::new());

    // Build cache system with DashMapCache as L1
    let cache = CacheSystemBuilder::new()
        .with_l1(dashmap_l1.clone() as Arc<dyn CacheBackend>)
        .build()
        .await?;

    let manager = cache.cache_manager();

    // Test operations
    let test_data = serde_json::json!({
        "user": "bob",
        "role": "admin",
        "permissions": ["read", "write", "delete"]
    });

    manager.set_with_strategy(
        "user:bob",
        test_data.clone(),
        CacheStrategy::ShortTerm,
    ).await?;

    if let Some(cached) = manager.get("user:bob").await? {
        println!("✅ Retrieved from DashMapCache: {}", cached);
    }

    // Show statistics
    let stats = manager.get_stats();
    println!("📊 Stats - L1 hits: {}, L2 hits: {}, misses: {}",
        stats.l1_hits, stats.l2_hits, stats.misses);

    // Demonstrate cleanup of expired entries
    println!("\n🧹 DashMapCache has manual cleanup (no automatic eviction)");
    let removed = dashmap_l1.cleanup_expired();
    println!("   Cleaned up {} expired entries", removed);

    Ok(())
}

/// Demonstrate MemcachedCache standalone usage
#[cfg(feature = "backend-memcached")]
async fn demo_memcached_backend() -> Result<()> {
    use multi_tier_cache::MemcachedCache;
    use std::time::Duration;

    println!("Using MemcachedCache (standalone demonstration)...");
    println!("⚠️  Note: Requires Memcached server running at localhost:11211");

    // Try to create MemcachedCache (may fail if server not running)
    match MemcachedCache::new().await {
        Ok(memcached) => {
            println!("✅ Connected to Memcached");

            // Test direct operations with MemcachedCache
            let test_data = serde_json::json!({
                "product": "laptop",
                "price": 999.99,
                "stock": 42
            });

            // Set with TTL
            memcached.set_with_ttl(
                "product:laptop",
                test_data.clone(),
                Duration::from_secs(300),
            ).await?;

            // Get the value
            if let Some(cached) = memcached.get("product:laptop").await {
                println!("✅ Retrieved from MemcachedCache: {}", cached);
            }

            // Show server statistics
            if let Ok(server_stats) = memcached.get_server_stats() {
                println!("\n📊 Memcached Server Stats:");
                for (server, stats) in server_stats {
                    println!("   Server: {}", server);
                    if let Some(version) = stats.get("version") {
                        println!("     Version: {}", version);
                    }
                    if let Some(uptime) = stats.get("uptime") {
                        println!("     Uptime: {}s", uptime);
                    }
                    if let Some(cmd_get) = stats.get("cmd_get") {
                        println!("     Total GETs: {}", cmd_get);
                    }
                    if let Some(cmd_set) = stats.get("cmd_set") {
                        println!("     Total SETs: {}", cmd_set);
                    }
                }
            }

            // Clean up
            memcached.remove("product:laptop").await?;

            println!("\n💡 Note: MemcachedCache implements CacheBackend but not L2CacheBackend");
            println!("   This is because Memcached doesn't support TTL introspection.");
            println!("   You can use it standalone or wrap it in a custom backend that implements L2CacheBackend.");
        }
        Err(e) => {
            println!("❌ Failed to connect to Memcached: {}", e);
            println!("   Make sure Memcached is running: memcached -p 11211");
            println!("   Or set MEMCACHED_URL environment variable");
        }
    }

    Ok(())
}

/// Demonstrate QuickCacheBackend as L1 backend
#[cfg(feature = "backend-quickcache")]
async fn demo_quickcache_backend() -> Result<()> {
    use multi_tier_cache::QuickCacheBackend;

    println!("Using QuickCacheBackend as L1 backend...");

    // Create QuickCacheBackend with custom capacity
    let quickcache_l1 = Arc::new(QuickCacheBackend::new(5000).await?);

    // Build cache system with QuickCache as L1
    let cache = CacheSystemBuilder::new()
        .with_l1(quickcache_l1.clone() as Arc<dyn CacheBackend>)
        .build()
        .await?;

    let manager = cache.cache_manager();

    // Test operations with high-performance cache
    let test_data = serde_json::json!({
        "session_id": "abc123",
        "user_id": 42,
        "expires_at": 1234567890
    });

    manager.set_with_strategy(
        "session:abc123",
        test_data.clone(),
        CacheStrategy::ShortTerm,
    ).await?;

    if let Some(cached) = manager.get("session:abc123").await? {
        println!("✅ Retrieved from QuickCache: {}", cached);
    }

    // Show statistics
    let stats = manager.get_stats();
    println!("📊 Stats - L1 hits: {}, L2 hits: {}, misses: {}",
        stats.l1_hits, stats.l2_hits, stats.misses);

    println!("\n⚡ QuickCache is optimized for maximum throughput");
    println!("   Use it when you need sub-microsecond latency");

    Ok(())
}
