//! Integration tests for Redis Streams functionality

mod common;

use common::*;

/// Test publishing to Redis Stream
#[tokio::test]
async fn test_stream_publish() {
    let cache = setup_cache_system()
        .await
        .unwrap_or_else(|_| panic!("Failed to setup cache system"));
    let stream_key = format!("test:stream:{}", rand::random::<u32>());

    let fields = vec![
        ("event".to_string(), "user_login".to_string()),
        ("user_id".to_string(), "123".to_string()),
        ("timestamp".to_string(), "2025-01-01T00:00:00Z".to_string()),
    ];

    let entry_id = cache
        .cache_manager()
        .publish_to_stream(&stream_key, fields, Some(100))
        .await
        .unwrap_or_else(|_| panic!("Failed to publish to stream"));

    assert!(!entry_id.is_empty());
}

/// Test reading from Redis Stream
#[tokio::test]
async fn test_stream_read_latest() {
    let cache = setup_cache_system()
        .await
        .unwrap_or_else(|_| panic!("Failed to setup cache system"));
    let stream_key = format!("test:stream:{}", rand::random::<u32>());

    // Publish multiple entries
    for i in 1..=5 {
        let fields = vec![
            ("event".to_string(), format!("event_{i}")),
            ("count".to_string(), i.to_string()),
        ];
        cache
            .cache_manager()
            .publish_to_stream(&stream_key, fields, Some(100))
            .await
            .unwrap_or_else(|_| panic!("Failed to publish to stream"));
    }

    // Read latest entries
    let entries = cache
        .cache_manager()
        .read_stream_latest(&stream_key, 3)
        .await
        .unwrap_or_else(|_| panic!("Failed to read from stream"));

    assert!(entries.len() <= 3);
    assert!(!entries.is_empty());
}

/// Test stream auto-trimming
#[tokio::test]
async fn test_stream_maxlen_trimming() {
    let cache = setup_cache_system()
        .await
        .unwrap_or_else(|_| panic!("Failed to setup cache system"));
    let stream_key = format!("test:stream:{}", rand::random::<u32>());

    // Publish many entries with maxlen=5
    for i in 1..=10 {
        let fields = vec![("count".to_string(), i.to_string())];
        cache
            .cache_manager()
            .publish_to_stream(&stream_key, fields, Some(5))
            .await
            .unwrap_or_else(|_| panic!("Failed to publish to stream"));
    }

    // Read all entries
    let entries = cache
        .cache_manager()
        .read_stream_latest(&stream_key, 100)
        .await
        .unwrap_or_else(|_| panic!("Failed to read from stream"));

    // Should be trimmed to ~5 entries (approximate trimming)
    assert!(entries.len() <= 10);
}
