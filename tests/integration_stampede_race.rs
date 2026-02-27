//! Stampede protection regression tests.
//!
//! ## Analysis of the TOCTOU concern
//!
//! The original code review (§1.2) noted a potential TOCTOU race where
//! `CleanupGuard` removes the DashMap entry before mutex waiters wake up,
//! allowing a new request to create a new mutex and start duplicate work.
//!
//! After thorough analysis, **this is NOT exploitable in `get_or_compute_with`**:
//!
//! 1. The mutex `_guard` is held through compute + `set_with_strategy`
//! 2. `_cleanup_guard` drops first (Rust drops in reverse declaration order)
//! 3. Then `_guard` drops, releasing the mutex
//! 4. By that point the value is already in cache — any new request hits L1
//!
//! In `get()`, the race can cause redundant L2 lookups and L1 promotions,
//! but NOT duplicate computation (there is no compute in `get()`).
//!
//! These tests serve as **regression tests** to ensure the stampede protection
//! guarantees are maintained as the code evolves.

use anyhow::Result;
use multi_tier_cache::{
    async_trait, CacheBackend, CacheManager, CacheStrategy, CacheTier, L2CacheBackend,
};
use serde_json::json;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Barrier;

// =============================================================================
// Test backends
// =============================================================================

/// Simple in-memory backend for testing.
struct InMemoryBackend {
    store: dashmap::DashMap<String, serde_json::Value>,
}

impl InMemoryBackend {
    fn new() -> Self {
        Self {
            store: dashmap::DashMap::new(),
        }
    }
}

#[async_trait]
impl CacheBackend for InMemoryBackend {
    async fn get(&self, key: &str) -> Option<serde_json::Value> {
        self.store.get(key).map(|v| v.value().clone())
    }

    async fn set_with_ttl(
        &self,
        key: &str,
        value: serde_json::Value,
        _ttl: Duration,
    ) -> Result<()> {
        self.store.insert(key.to_string(), value);
        Ok(())
    }

    async fn remove(&self, key: &str) -> Result<()> {
        self.store.remove(key);
        Ok(())
    }

    async fn health_check(&self) -> bool {
        true
    }

    fn name(&self) -> &'static str {
        "InMemoryBackend"
    }
}

#[async_trait]
impl L2CacheBackend for InMemoryBackend {
    async fn get_with_ttl(&self, key: &str) -> Option<(serde_json::Value, Option<Duration>)> {
        self.store
            .get(key)
            .map(|v| (v.value().clone(), Some(Duration::from_secs(300))))
    }
}

/// Backend with configurable write delay to test edge cases.
struct SlowWriteBackend {
    store: dashmap::DashMap<String, serde_json::Value>,
    slow_writes: Arc<AtomicBool>,
    write_delay: Duration,
}

impl SlowWriteBackend {
    fn new(slow_writes: Arc<AtomicBool>, write_delay: Duration) -> Self {
        Self {
            store: dashmap::DashMap::new(),
            slow_writes,
            write_delay,
        }
    }
}

#[async_trait]
impl CacheBackend for SlowWriteBackend {
    async fn get(&self, key: &str) -> Option<serde_json::Value> {
        self.store.get(key).map(|v| v.value().clone())
    }

    async fn set_with_ttl(
        &self,
        key: &str,
        value: serde_json::Value,
        _ttl: Duration,
    ) -> Result<()> {
        if self.slow_writes.load(Ordering::SeqCst) {
            tokio::time::sleep(self.write_delay).await;
        }
        self.store.insert(key.to_string(), value);
        Ok(())
    }

    async fn remove(&self, key: &str) -> Result<()> {
        self.store.remove(key);
        Ok(())
    }

    async fn health_check(&self) -> bool {
        true
    }

    fn name(&self) -> &'static str {
        "SlowWriteBackend"
    }
}

#[async_trait]
impl L2CacheBackend for SlowWriteBackend {
    async fn get_with_ttl(&self, key: &str) -> Option<(serde_json::Value, Option<Duration>)> {
        self.store
            .get(key)
            .map(|v| (v.value().clone(), Some(Duration::from_secs(300))))
    }
}

// =============================================================================
// Helpers
// =============================================================================

fn build_manager(l1: Arc<dyn L2CacheBackend>, l2: Arc<dyn L2CacheBackend>) -> Arc<CacheManager> {
    let tiers = vec![
        CacheTier::new(l1, 1, false, 1.0),
        CacheTier::new(l2, 2, true, 1.0),
    ];
    Arc::new(CacheManager::new_with_tiers(tiers, None).expect("CacheManager"))
}

// =============================================================================
// Tests
// =============================================================================

/// Concurrent requests for the same key should result in exactly one computation.
#[tokio::test]
async fn test_stampede_single_compute_concurrent_requests() {
    let compute_count = Arc::new(AtomicUsize::new(0));
    let manager = build_manager(
        Arc::new(InMemoryBackend::new()),
        Arc::new(InMemoryBackend::new()),
    );

    let key = "stampede:concurrent";
    let n = 20;
    let barrier = Arc::new(Barrier::new(n + 1));
    let mut handles = Vec::new();

    for _ in 0..n {
        let mgr = Arc::clone(&manager);
        let counter = Arc::clone(&compute_count);
        let b = Arc::clone(&barrier);

        handles.push(tokio::spawn(async move {
            b.wait().await;
            mgr.get_or_compute_with(key, CacheStrategy::ShortTerm, || {
                let c = counter;
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    Ok(json!({"computed": true}))
                }
            })
            .await
        }));
    }

    barrier.wait().await;

    for handle in handles {
        let result = handle.await.expect("task panicked");
        assert!(result.is_ok());
    }

    assert_eq!(
        compute_count.load(Ordering::SeqCst),
        1,
        "Compute should be called exactly once under concurrent load"
    );
}

/// After one request computes and caches the value, subsequent requests
/// should always hit cache — never recompute.
#[tokio::test]
async fn test_stampede_no_recompute_after_cached() {
    let compute_count = Arc::new(AtomicUsize::new(0));
    let manager = build_manager(
        Arc::new(InMemoryBackend::new()),
        Arc::new(InMemoryBackend::new()),
    );

    let key = "stampede:cached";

    // First: compute the value
    {
        let counter = Arc::clone(&compute_count);
        manager
            .get_or_compute_with(key, CacheStrategy::ShortTerm, || {
                let c = counter;
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    Ok(json!({"first": true}))
                }
            })
            .await
            .expect("first compute");
    }

    assert_eq!(compute_count.load(Ordering::SeqCst), 1);

    // Then: blast concurrent requests — all should hit cache
    let n = 50;
    let barrier = Arc::new(Barrier::new(n + 1));
    let mut handles = Vec::new();

    for _ in 0..n {
        let mgr = Arc::clone(&manager);
        let counter = Arc::clone(&compute_count);
        let b = Arc::clone(&barrier);

        handles.push(tokio::spawn(async move {
            b.wait().await;
            mgr.get_or_compute_with(key, CacheStrategy::ShortTerm, || {
                let c = counter;
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    Ok(json!({"recomputed": true}))
                }
            })
            .await
        }));
    }

    barrier.wait().await;

    for handle in handles {
        assert!(handle.await.expect("task panicked").is_ok());
    }

    assert_eq!(
        compute_count.load(Ordering::SeqCst),
        1,
        "Value was already cached — compute should not be called again"
    );
}

/// Two waves of requests: wave 2 arrives while wave 1 is computing.
/// Even with wave overlap, compute should be called exactly once.
#[tokio::test]
async fn test_stampede_two_waves_single_compute() {
    let compute_count = Arc::new(AtomicUsize::new(0));
    let manager = build_manager(
        Arc::new(InMemoryBackend::new()),
        Arc::new(InMemoryBackend::new()),
    );

    let key = "stampede:waves";
    let mut handles = Vec::new();

    // Wave 1: 10 tasks
    let barrier1 = Arc::new(Barrier::new(11));
    for _ in 0..10 {
        let mgr = Arc::clone(&manager);
        let counter = Arc::clone(&compute_count);
        let b = Arc::clone(&barrier1);

        handles.push(tokio::spawn(async move {
            b.wait().await;
            mgr.get_or_compute_with(key, CacheStrategy::ShortTerm, || {
                let c = counter;
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    Ok(json!({"wave": 1}))
                }
            })
            .await
        }));
    }

    barrier1.wait().await;

    // Wait a bit, then launch wave 2 while wave 1 is still computing
    tokio::time::sleep(Duration::from_millis(20)).await;

    for _ in 0..10 {
        let mgr = Arc::clone(&manager);
        let counter = Arc::clone(&compute_count);

        handles.push(tokio::spawn(async move {
            mgr.get_or_compute_with(key, CacheStrategy::ShortTerm, || {
                let c = counter;
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    Ok(json!({"wave": 2}))
                }
            })
            .await
        }));
    }

    for handle in handles {
        assert!(handle.await.expect("task panicked").is_ok());
    }

    assert_eq!(
        compute_count.load(Ordering::SeqCst),
        1,
        "Overlapping waves should still result in exactly one computation"
    );
}

/// Even with slow L1 writes, stampede protection should prevent duplicate
/// computation because the mutex is held through the entire set_with_strategy call.
#[tokio::test]
async fn test_stampede_safe_with_slow_writes() {
    let compute_count = Arc::new(AtomicUsize::new(0));
    let slow_writes = Arc::new(AtomicBool::new(true));

    let manager = build_manager(
        Arc::new(SlowWriteBackend::new(
            Arc::clone(&slow_writes),
            Duration::from_millis(100),
        )),
        Arc::new(SlowWriteBackend::new(
            Arc::clone(&slow_writes),
            Duration::from_millis(100),
        )),
    );

    let key = "stampede:slow";

    // Wave 1
    let barrier = Arc::new(Barrier::new(6));
    let mut handles = Vec::new();

    for _ in 0..5 {
        let mgr = Arc::clone(&manager);
        let counter = Arc::clone(&compute_count);
        let b = Arc::clone(&barrier);

        handles.push(tokio::spawn(async move {
            b.wait().await;
            mgr.get_or_compute_with(key, CacheStrategy::ShortTerm, || {
                let c = counter;
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    Ok(json!({"computed": true}))
                }
            })
            .await
        }));
    }

    barrier.wait().await;

    // Wave 2: arrives while wave 1's set_with_strategy is doing slow writes
    tokio::time::sleep(Duration::from_millis(80)).await;

    for _ in 0..5 {
        let mgr = Arc::clone(&manager);
        let counter = Arc::clone(&compute_count);

        handles.push(tokio::spawn(async move {
            mgr.get_or_compute_with(key, CacheStrategy::ShortTerm, || {
                let c = counter;
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    Ok(json!({"duplicate": true}))
                }
            })
            .await
        }));
    }

    for handle in handles {
        assert!(handle.await.expect("task panicked").is_ok());
    }

    assert_eq!(
        compute_count.load(Ordering::SeqCst),
        1,
        "Mutex is held through set_with_strategy, so slow writes should not \
         cause duplicate computation"
    );
}

/// Multiple distinct keys should each be computed exactly once,
/// even under high concurrency.
#[tokio::test]
async fn test_stampede_multi_key_isolation() {
    let compute_counts: Arc<dashmap::DashMap<String, AtomicUsize>> =
        Arc::new(dashmap::DashMap::new());

    let manager = build_manager(
        Arc::new(InMemoryBackend::new()),
        Arc::new(InMemoryBackend::new()),
    );

    let num_keys = 5;
    let requests_per_key = 20;
    let total = num_keys * requests_per_key;
    let barrier = Arc::new(Barrier::new(total + 1));
    let mut handles = Vec::new();

    for key_id in 0..num_keys {
        for _ in 0..requests_per_key {
            let mgr = Arc::clone(&manager);
            let counts = Arc::clone(&compute_counts);
            let b = Arc::clone(&barrier);

            handles.push(tokio::spawn(async move {
                let key = format!("stampede:multi:{key_id}");
                b.wait().await;

                mgr.get_or_compute_with(&key, CacheStrategy::ShortTerm, || {
                    let k = key.clone();
                    let c = Arc::clone(&counts);
                    async move {
                        c.entry(k)
                            .or_insert_with(|| AtomicUsize::new(0))
                            .fetch_add(1, Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(20)).await;
                        Ok(json!({"key_id": key_id}))
                    }
                })
                .await
            }));
        }
    }

    barrier.wait().await;

    for handle in handles {
        assert!(handle.await.expect("task panicked").is_ok());
    }

    for key_id in 0..num_keys {
        let key = format!("stampede:multi:{key_id}");
        let count = compute_counts
            .get(&key)
            .map(|v| v.load(Ordering::SeqCst))
            .unwrap_or(0);

        assert_eq!(count, 1, "Key '{key}' computed {count} times (expected 1)");
    }
}
