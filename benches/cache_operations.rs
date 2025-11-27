//! Benchmarks for basic cache operations
//!
//! This benchmark suite measures the performance of:
//! - L1 cache read/write operations
//! - L2 cache read/write operations
//! - Combined L1+L2 operations
//! - Cache hit vs miss latency
//! - Different data sizes

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use multi_tier_cache::{CacheBackend, CacheStrategy, CacheSystem};
use serde_json::json;
use std::time::Duration;
use tokio::runtime::Runtime;

/// Setup cache system for benchmarks
fn setup_cache() -> (CacheSystem, Runtime) {
    let rt = Runtime::new().unwrap_or_else(|_| panic!("Failed to create runtime"));
    let cache = rt.block_on(async {
        std::env::set_var("REDIS_URL", "redis://127.0.0.1:6379");
        CacheSystem::new()
            .await
            .unwrap_or_else(|_| panic!("Failed to create cache system"))
    });
    (cache, rt)
}

/// Generate test data of specified size
fn test_data(size_bytes: usize) -> serde_json::Value {
    let data_string = "x".repeat(size_bytes);
    json!({
        "data": data_string,
        "size": size_bytes,
        "timestamp": "2025-01-01T00:00:00Z"
    })
}

/// Benchmark L1 + L2 cache write operations
fn bench_cache_set(c: &mut Criterion) {
    let (cache, rt) = setup_cache();

    let mut group = c.benchmark_group("cache_set");
    group.measurement_time(Duration::from_secs(10));

    for size in &[100, 1024, 10240, 102_400] {
        let data = test_data(*size);

        group.bench_with_input(BenchmarkId::new("short_term", size), size, |b, _| {
            b.iter(|| {
                rt.block_on(async {
                    let key = format!("bench:set:{}", rand::random::<u32>());
                    cache
                        .cache_manager()
                        .set_with_strategy(&key, black_box(data.clone()), CacheStrategy::ShortTerm)
                        .await
                        .unwrap_or_else(|_| panic!("Failed to set cache"));
                });
            });
        });

        group.bench_with_input(BenchmarkId::new("long_term", size), size, |b, _| {
            b.iter(|| {
                rt.block_on(async {
                    let key = format!("bench:set:{}", rand::random::<u32>());
                    cache
                        .cache_manager()
                        .set_with_strategy(&key, black_box(data.clone()), CacheStrategy::LongTerm)
                        .await
                        .unwrap_or_else(|_| panic!("Failed to set cache"));
                });
            });
        });
    }

    group.finish();
}

/// Benchmark L1 cache hit performance
fn bench_l1_hit(c: &mut Criterion) {
    let (cache, rt) = setup_cache();

    // Pre-populate cache
    rt.block_on(async {
        for i in 0..100 {
            let key = format!("bench:l1:{i}");
            cache
                .cache_manager()
                .set_with_strategy(&key, test_data(1024), CacheStrategy::ShortTerm)
                .await
                .unwrap_or_else(|_| panic!("Failed to set cache"));
            // Warm up L1
            let _ = cache
                .cache_manager()
                .get(&key)
                .await
                .unwrap_or_else(|_| panic!("Failed to get cache"));
        }
    });

    c.bench_function("l1_cache_hit", |b| {
        b.iter(|| {
            rt.block_on(async {
                let key = format!("bench:l1:{}", rand::random::<u8>() % 100);
                black_box(
                    cache
                        .cache_manager()
                        .get(&key)
                        .await
                        .unwrap_or_else(|_| panic!("Failed to get cache")),
                );
            });
        });
    });
}

/// Benchmark L2 cache hit performance (L1 miss)
fn bench_l2_hit(c: &mut Criterion) {
    let (cache, rt) = setup_cache();

    // Pre-populate L2 only
    rt.block_on(async {
        for i in 0..100 {
            let key = format!("bench:l2:{i}");
            if let Some(l2) = &cache.l2_cache {
                l2.set_with_ttl(&key, test_data(1024), Duration::from_secs(300))
                    .await
                    .unwrap_or_else(|_| panic!("Failed to set cache"));
            }
        }
    });

    c.bench_function("l2_cache_hit", |b| {
        b.iter(|| {
            rt.block_on(async {
                let key = format!("bench:l2:{}", rand::random::<u8>() % 100);
                // Clear L1 to force L2 access
                if let Some(l1) = &cache.l1_cache {
                    l1.remove(&key)
                        .await
                        .unwrap_or_else(|_| panic!("Failed to remove from L1"));
                }
                black_box(
                    cache
                        .cache_manager()
                        .get(&key)
                        .await
                        .unwrap_or_else(|_| panic!("Failed to get cache")),
                );
            });
        });
    });
}

/// Benchmark cache miss performance
fn bench_cache_miss(c: &mut Criterion) {
    let (cache, rt) = setup_cache();

    c.bench_function("cache_miss", |b| {
        b.iter(|| {
            rt.block_on(async {
                let key = format!("bench:miss:{}", rand::random::<u32>());
                black_box(
                    cache
                        .cache_manager()
                        .get(&key)
                        .await
                        .unwrap_or_else(|_| panic!("Failed to get cache")),
                );
            });
        });
    });
}

/// Benchmark compute-on-miss pattern
fn bench_compute_on_miss(c: &mut Criterion) {
    let (cache, rt) = setup_cache();

    let mut group = c.benchmark_group("compute_on_miss");

    // Simulate different computation latencies
    for delay_ms in &[1, 10, 50] {
        let delay = Duration::from_millis(*delay_ms);

        group.bench_with_input(BenchmarkId::from_parameter(delay_ms), delay_ms, |b, _| {
            b.iter(|| {
                rt.block_on(async {
                    let key = format!("bench:compute:{}", rand::random::<u32>());
                    let data = test_data(1024);

                    cache
                        .cache_manager()
                        .get_or_compute_with(&key, CacheStrategy::ShortTerm, || {
                            let d = data.clone();
                            async move {
                                tokio::time::sleep(delay).await;
                                Ok(d)
                            }
                        })
                        .await
                        .unwrap_or_else(|_| panic!("Failed to get/compute"));
                });
            });
        });
    }

    group.finish();
}

/// Benchmark type-safe caching with serialization
fn bench_typed_cache(c: &mut Criterion) {
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct User {
        id: u64,
        name: String,
        email: String,
        profile: String,
    }

    let (cache, rt) = setup_cache();

    c.bench_function("typed_cache_set_get", |b| {
        b.iter(|| {
            rt.block_on(async {
                let key = format!("bench:typed:{}", rand::random::<u32>());
                let user = User {
                    id: 123,
                    name: "Test User".to_string(),
                    email: "test@example.com".to_string(),
                    profile: "x".repeat(1024),
                };

                // Set
                cache
                    .cache_manager()
                    .get_or_compute_typed(&key, CacheStrategy::ShortTerm, || {
                        let u = user.clone();
                        async move { Ok(u) }
                    })
                    .await
                    .unwrap_or_else(|_| panic!("Failed to get/compute typed"));

                // Get
                black_box(
                    cache
                        .cache_manager()
                        .get_or_compute_typed::<User, _, _>(
                            &key,
                            CacheStrategy::ShortTerm,
                            || async {
                                panic!("Should not compute");
                            },
                        )
                        .await
                        .unwrap_or_else(|_| panic!("Failed to get/compute typed")),
                );
            });
        });
    });
}

/// Benchmark different cache strategies
fn bench_cache_strategies(c: &mut Criterion) {
    let (cache, rt) = setup_cache();

    let mut group = c.benchmark_group("cache_strategies");
    let data = test_data(1024);

    let strategies = vec![
        ("realtime", CacheStrategy::RealTime),
        ("short_term", CacheStrategy::ShortTerm),
        ("medium_term", CacheStrategy::MediumTerm),
        ("long_term", CacheStrategy::LongTerm),
        ("custom", CacheStrategy::Custom(Duration::from_secs(60))),
    ];

    for (name, strategy) in &strategies {
        group.bench_function(*name, |b| {
            b.iter(|| {
                rt.block_on(async {
                    let key = format!("bench:strategy:{}", rand::random::<u32>());
                    cache
                        .cache_manager()
                        .set_with_strategy(&key, black_box(data.clone()), strategy.clone())
                        .await
                        .unwrap_or_else(|_| panic!("Failed to set cache"));
                });
            });
        });
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_cache_set,
    bench_l1_hit,
    bench_l2_hit,
    bench_cache_miss,
    bench_compute_on_miss,
    bench_typed_cache,
    bench_cache_strategies
);
criterion_main!(benches);
