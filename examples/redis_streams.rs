//! Redis Streams Example
//!
//! Demonstrates publishing and consuming data via Redis Streams.
//!
//! Run with: cargo run --example redis_streams

use multi_tier_cache::CacheSystem;
use std::time::{SystemTime, UNIX_EPOCH};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("=== Multi-Tier Cache: Redis Streams ===\n");

    // Initialize cache system
    let cache = CacheSystem::new().await?;
    println!();

    // 1. Publish events to stream
    println!("Publishing events to 'events_stream'...\n");

    for i in 1..=5 {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let fields = vec![
            ("event_id".to_string(), i.to_string()),
            ("event_type".to_string(), "user_action".to_string()),
            ("user_id".to_string(), format!("user_{}", i)),
            ("timestamp".to_string(), timestamp.to_string()),
            ("action".to_string(), "login".to_string()),
        ];

        let entry_id = cache
            .cache_manager()
            .publish_to_stream("events_stream", fields, Some(1000))
            .await?;

        println!("  ✅ Published event {} with ID: {}", i, entry_id);
    }

    println!();

    // 2. Read latest entries from stream
    println!("Reading latest 3 entries from stream...\n");

    let entries = cache
        .cache_manager()
        .read_stream_latest("events_stream", 3)
        .await?;

    for (entry_id, fields) in &entries {
        println!("  Entry ID: {}", entry_id);
        for (field, value) in fields {
            println!("    {}: {}", field, value);
        }
        println!();
    }

    // 3. Read stream from beginning (XREAD)
    println!("Reading all entries from stream (from beginning)...\n");

    let all_entries = cache
        .cache_manager()
        .read_stream("events_stream", "0", 10, None)
        .await?;

    println!("  Total entries: {}", all_entries.len());

    // 4. Publish more data (simulate real-time updates)
    println!("\nSimulating real-time event stream...\n");

    for i in 6..=10 {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let fields = vec![
            ("event_id".to_string(), i.to_string()),
            ("event_type".to_string(), "api_call".to_string()),
            ("endpoint".to_string(), "/api/data".to_string()),
            ("timestamp".to_string(), timestamp.to_string()),
            ("status_code".to_string(), "200".to_string()),
        ];

        cache
            .cache_manager()
            .publish_to_stream("events_stream", fields, Some(1000))
            .await?;

        println!("  📤 Published real-time event {}", i);
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    println!("\n=== Stream Summary ===");
    let final_entries = cache
        .cache_manager()
        .read_stream_latest("events_stream", 1000)
        .await?;
    println!("Total events in stream: {}", final_entries.len());
    println!("(Automatically trimmed to last 1000 entries)");

    Ok(())
}
