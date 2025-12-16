//! Benchmarks for multi-tier cache operations (v0.5.0+)

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use multi_tier_cache::{CacheStrategy, CacheSystem, CacheSystemBuilder, L2Cache, TierConfig};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Runtime;

/// Generate test data of specified size
fn test_data(size_bytes: usize) -> serde_json::Value {
    let data_string = "x".repeat(size_bytes);
    json!({
        "data": data_string,
        "size": size_bytes,
        "timestamp": "2025-01-01T00:00:00Z"
    })
}

/// Helper to build a 2-tier cache system
fn build_2tier_cache(rt: &Runtime) -> CacheSystem {
    rt.block_on(async {
        let l1 = Arc::new(
            L2Cache::new()
                .await
                .unwrap_or_else(|_| panic!("Failed to create L1")),
        );
        let l2 = Arc::new(
            L2Cache::new()
                .await
                .unwrap_or_else(|_| panic!("Failed to create L2")),
        );

        CacheSystemBuilder::new()
            .with_tier(l1, TierConfig::as_l1())
            .with_tier(l2, TierConfig::as_l2())
            .build()
            .await
            .unwrap_or_else(|_| panic!("Failed to build cache system"))
    })
}

/// Helper to build a 3-tier cache system
fn build_3tier_cache(rt: &Runtime) -> CacheSystem {
    rt.block_on(async {
        let l1 = Arc::new(
            L2Cache::new()
                .await
                .unwrap_or_else(|_| panic!("Failed to create L1")),
        );
        let l2 = Arc::new(
            L2Cache::new()
                .await
                .unwrap_or_else(|_| panic!("Failed to create L2")),
        );
        let l3 = Arc::new(
            L2Cache::new()
                .await
                .unwrap_or_else(|_| panic!("Failed to create L3")),
        );

        CacheSystemBuilder::new()
            .with_tier(l1, TierConfig::as_l1())
            .with_tier(l2, TierConfig::as_l2())
            .with_tier(l3, TierConfig::as_l3())
            .build()
            .await
            .unwrap_or_else(|_| panic!("Failed to build cache system"))
    })
}

/// Helper to build a 4-tier cache system
fn build_4tier_cache(rt: &Runtime) -> CacheSystem {
    rt.block_on(async {
        let l1 = Arc::new(
            L2Cache::new()
                .await
                .unwrap_or_else(|_| panic!("Failed to create L1")),
        );
        let l2 = Arc::new(
            L2Cache::new()
                .await
                .unwrap_or_else(|_| panic!("Failed to create L2")),
        );
        let l3 = Arc::new(
            L2Cache::new()
                .await
                .unwrap_or_else(|_| panic!("Failed to create L3")),
        );
        let l4 = Arc::new(
            L2Cache::new()
                .await
                .unwrap_or_else(|_| panic!("Failed to create L4")),
        );

        CacheSystemBuilder::new()
            .with_tier(l1, TierConfig::as_l1())
            .with_tier(l2, TierConfig::as_l2())
            .with_tier(l3, TierConfig::as_l3())
            .with_tier(l4, TierConfig::as_l4())
            .build()
            .await
            .unwrap_or_else(|_| panic!("Failed to build cache system"))
    })
}

/// Benchmark 2-tier vs 3-tier vs 4-tier write performance
fn bench_multi_tier_write(c: &mut Criterion) {
    let rt = Runtime::new().unwrap_or_else(|_| panic!("Failed to create runtime"));

    let mut group = c.benchmark_group("multi_tier_write");
    group.measurement_time(Duration::from_secs(10));

    let test_val = test_data(1024);

    // 2-tier benchmark (baseline)
    let cache_2tier = build_2tier_cache(&rt);

    group.bench_function("2_tiers", |b| {
        b.iter(|| {
            rt.block_on(async {
                let key = format!("bench:mt2:{}", rand::random::<u32>());
                cache_2tier
                    .cache_manager()
                    .set_with_strategy(&key, &black_box(test_val.clone()), CacheStrategy::ShortTerm)
                    .await
                    .unwrap_or_else(|_| panic!("Failed to set cache"));
            });
        });
    });

    // 3-tier benchmark
    let cache_3tier = build_3tier_cache(&rt);

    group.bench_function("3_tiers", |b| {
        b.iter(|| {
            rt.block_on(async {
                let key = format!("bench:mt3:{}", rand::random::<u32>());
                cache_3tier
                    .cache_manager()
                    .set_with_strategy(&key, &black_box(test_val.clone()), CacheStrategy::ShortTerm)
                    .await
                    .unwrap_or_else(|_| panic!("Failed to set cache"));
            });
        });
    });

    // 4-tier benchmark
    let cache_4tier = build_4tier_cache(&rt);

    group.bench_function("4_tiers", |b| {
        b.iter(|| {
            rt.block_on(async {
                let key = format!("bench:mt4:{}", rand::random::<u32>());
                cache_4tier
                    .cache_manager()
                    .set_with_strategy(&key, &black_box(test_val.clone()), CacheStrategy::ShortTerm)
                    .await
                    .unwrap_or_else(|_| panic!("Failed to set cache"));
            });
        });
    });

    group.finish();
}

/// Benchmark multi-tier read performance (L1 hits)
fn bench_multi_tier_read(c: &mut Criterion) {
    let rt = Runtime::new().unwrap_or_else(|_| panic!("Failed to create runtime"));

    let cache = rt.block_on(async {
        let l1 = Arc::new(
            L2Cache::new()
                .await
                .unwrap_or_else(|_| panic!("Failed to create L1")),
        );
        let l2 = Arc::new(
            L2Cache::new()
                .await
                .unwrap_or_else(|_| panic!("Failed to create L2")),
        );
        let l3 = Arc::new(
            L2Cache::new()
                .await
                .unwrap_or_else(|_| panic!("Failed to create L3")),
        );

        CacheSystemBuilder::new()
            .with_tier(l1, TierConfig::as_l1())
            .with_tier(l2, TierConfig::as_l2())
            .with_tier(l3, TierConfig::as_l3())
            .build()
            .await
            .unwrap_or_else(|_| panic!("Failed to build cache system"))
    });

    // Pre-populate cache
    rt.block_on(async {
        for i in 0..100 {
            let key = format!("bench:read:{i}");
            cache
                .cache_manager()
                .set_with_strategy(&key, &test_data(1024), CacheStrategy::ShortTerm)
                .await
                .unwrap_or_else(|_| panic!("Failed to set cache"));
        }
    });

    c.bench_function("multi_tier_l1_hit", |b| {
        b.iter(|| {
            rt.block_on(async {
                let key = format!("bench:read:{}", rand::random::<u8>() % 100);
                black_box(
                    cache
                        .cache_manager()
                        .get::<serde_json::Value>(&key)
                        .await
                        .unwrap_or_else(|_| panic!("Failed to get cache")),
                );
            });
        });
    });
}

/// Benchmark TTL scaling impact
fn bench_ttl_scaling(c: &mut Criterion) {
    let rt = Runtime::new().unwrap_or_else(|_| panic!("Failed to create runtime"));

    let mut group = c.benchmark_group("ttl_scaling");

    let test_val = test_data(1024);

    // Without scaling (all 1.0x)
    let cache_no_scale = rt.block_on(async {
        let l1 = Arc::new(
            L2Cache::new()
                .await
                .unwrap_or_else(|_| panic!("Failed to create L1")),
        );
        let l2 = Arc::new(
            L2Cache::new()
                .await
                .unwrap_or_else(|_| panic!("Failed to create L2")),
        );
        let l3 = Arc::new(
            L2Cache::new()
                .await
                .unwrap_or_else(|_| panic!("Failed to create L3")),
        );

        CacheSystemBuilder::new()
            .with_tier(l1, TierConfig::as_l1())
            .with_tier(l2, TierConfig::as_l2())
            .with_tier(l3, TierConfig::new(3).with_ttl_scale(1.0))
            .build()
            .await
            .unwrap_or_else(|_| panic!("Failed to build cache system"))
    });

    group.bench_function("no_scaling", |b| {
        b.iter(|| {
            rt.block_on(async {
                let key = format!("bench:scale:no:{}", rand::random::<u32>());
                cache_no_scale
                    .cache_manager()
                    .set_with_strategy(&key, &black_box(test_val.clone()), CacheStrategy::ShortTerm)
                    .await
                    .unwrap_or_else(|_| panic!("Failed to set cache"));
            });
        });
    });

    // With scaling (L3 = 2x, L4 = 8x)
    let cache_with_scale = rt.block_on(async {
        let l1 = Arc::new(
            L2Cache::new()
                .await
                .unwrap_or_else(|_| panic!("Failed to create L1")),
        );
        let l2 = Arc::new(
            L2Cache::new()
                .await
                .unwrap_or_else(|_| panic!("Failed to create L2")),
        );
        let l3 = Arc::new(
            L2Cache::new()
                .await
                .unwrap_or_else(|_| panic!("Failed to create L3")),
        );
        let l4 = Arc::new(
            L2Cache::new()
                .await
                .unwrap_or_else(|_| panic!("Failed to create L4")),
        );

        CacheSystemBuilder::new()
            .with_tier(l1, TierConfig::as_l1())
            .with_tier(l2, TierConfig::as_l2())
            .with_tier(l3, TierConfig::as_l3()) // 2x
            .with_tier(l4, TierConfig::as_l4()) // 8x
            .build()
            .await
            .unwrap_or_else(|_| panic!("Failed to build cache system"))
    });

    group.bench_function("with_scaling", |b| {
        b.iter(|| {
            rt.block_on(async {
                let key = format!("bench:scale:yes:{}", rand::random::<u32>());
                cache_with_scale
                    .cache_manager()
                    .set_with_strategy(&key, &black_box(test_val.clone()), CacheStrategy::ShortTerm)
                    .await
                    .unwrap_or_else(|_| panic!("Failed to set cache"));
            });
        });
    });

    group.finish();
}

/// Benchmark different data sizes across multi-tier
fn bench_data_size_multi_tier(c: &mut Criterion) {
    let rt = Runtime::new().unwrap_or_else(|_| panic!("Failed to create runtime"));

    let cache = rt.block_on(async {
        let l1 = Arc::new(
            L2Cache::new()
                .await
                .unwrap_or_else(|_| panic!("Failed to create L1")),
        );
        let l2 = Arc::new(
            L2Cache::new()
                .await
                .unwrap_or_else(|_| panic!("Failed to create L2")),
        );
        let l3 = Arc::new(
            L2Cache::new()
                .await
                .unwrap_or_else(|_| panic!("Failed to create L3")),
        );

        CacheSystemBuilder::new()
            .with_tier(l1, TierConfig::as_l1())
            .with_tier(l2, TierConfig::as_l2())
            .with_tier(l3, TierConfig::as_l3())
            .build()
            .await
            .unwrap_or_else(|_| panic!("Failed to build cache system"))
    });

    let mut group = c.benchmark_group("data_size_multi_tier");
    group.measurement_time(Duration::from_secs(10));

    for size in &[100, 1024, 10240, 102_400] {
        let data = test_data(*size);

        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, _| {
            b.iter(|| {
                rt.block_on(async {
                    let key = format!("bench:size:{}", rand::random::<u32>());
                    cache
                        .cache_manager()
                        .set_with_strategy(&key, &black_box(data.clone()), CacheStrategy::ShortTerm)
                        .await
                        .unwrap_or_else(|_| panic!("Failed to set cache"));

                    black_box(
                        cache
                            .cache_manager()
                            .get::<serde_json::Value>(&key)
                            .await
                            .unwrap_or_else(|_| panic!("Failed to get cache")),
                    );
                });
            });
        });
    }

    group.finish();
}

/// Benchmark tier statistics overhead
fn bench_tier_stats(c: &mut Criterion) {
    let rt = Runtime::new().unwrap_or_else(|_| panic!("Failed to create runtime"));

    let cache = rt.block_on(async {
        let l1 = Arc::new(
            L2Cache::new()
                .await
                .unwrap_or_else(|_| panic!("Failed to create L1")),
        );
        let l2 = Arc::new(
            L2Cache::new()
                .await
                .unwrap_or_else(|_| panic!("Failed to create L2")),
        );
        let l3 = Arc::new(
            L2Cache::new()
                .await
                .unwrap_or_else(|_| panic!("Failed to create L3")),
        );

        CacheSystemBuilder::new()
            .with_tier(l1, TierConfig::as_l1())
            .with_tier(l2, TierConfig::as_l2())
            .with_tier(l3, TierConfig::as_l3())
            .build()
            .await
            .unwrap_or_else(|_| panic!("Failed to build cache system"))
    });

    // Pre-populate
    rt.block_on(async {
        for i in 0..100 {
            let key = format!("bench:stats:{i}");
            cache
                .cache_manager()
                .set_with_strategy(&key, &test_data(1024), CacheStrategy::ShortTerm)
                .await
                .unwrap_or_else(|_| panic!("Failed to set cache"));
        }
    });

    c.bench_function("tier_stats_access", |b| {
        b.iter(|| {
            let stats = cache.cache_manager().get_tier_stats();
            black_box(stats);
        });
    });
}

criterion_group!(
    benches,
    bench_multi_tier_write,
    bench_multi_tier_read,
    bench_ttl_scaling,
    bench_data_size_multi_tier,
    bench_tier_stats
);
criterion_main!(benches);
