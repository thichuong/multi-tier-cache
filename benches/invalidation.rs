//! Benchmarks for cache invalidation operations

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use multi_tier_cache::{CacheManager, CacheStrategy, InvalidationConfig, L1Cache, L2Cache};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Runtime;

fn setup_cache_with_invalidation() -> (Arc<CacheManager>, Runtime) {
    let rt = Runtime::new().unwrap();
    let cache = rt.block_on(async {
        let redis_url =
            std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());

        let l1 = Arc::new(L1Cache::new().await.unwrap());
        let l2 = Arc::new(L2Cache::new().await.unwrap());
        let config = InvalidationConfig::default();

        Arc::new(
            CacheManager::new_with_invalidation(l1, l2, &redis_url, config)
                .await
                .expect("Failed to create cache manager with invalidation"),
        )
    });
    (cache, rt)
}

/// Benchmark single key invalidation
fn bench_invalidate_single_key(c: &mut Criterion) {
    let (cache, rt) = setup_cache_with_invalidation();

    // Pre-populate cache
    rt.block_on(async {
        for i in 0..100 {
            let key = format!("bench:inv:{}", i);
            cache
                .set_with_strategy(&key, json!({"id": i}), CacheStrategy::MediumTerm)
                .await
                .unwrap();
        }
    });

    c.bench_function("invalidate_single_key", |b| {
        b.iter(|| {
            rt.block_on(async {
                let key = format!("bench:inv:{}", rand::random::<u8>() % 100);
                black_box(cache.invalidate(&key).await.unwrap());
            })
        });
    });
}

/// Benchmark update cache operation
fn bench_update_cache(c: &mut Criterion) {
    let (cache, rt) = setup_cache_with_invalidation();

    rt.block_on(async {
        for i in 0..100 {
            let key = format!("bench:upd:{}", i);
            cache
                .set_with_strategy(&key, json!({"id": i}), CacheStrategy::MediumTerm)
                .await
                .unwrap();
        }
    });

    c.bench_function("update_cache", |b| {
        b.iter(|| {
            rt.block_on(async {
                let key = format!("bench:upd:{}", rand::random::<u8>() % 100);
                let new_value = json!({"id": 999, "value": "updated"});
                black_box(
                    cache
                        .update_cache(&key, new_value, Some(Duration::from_secs(300)))
                        .await
                        .unwrap(),
                );
            })
        });
    });
}

criterion_group!(benches, bench_invalidate_single_key, bench_update_cache);
criterion_main!(benches);
