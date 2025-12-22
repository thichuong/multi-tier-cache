//! Benchmarks for cache stampede protection

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use multi_tier_cache::{CacheStrategy, CacheSystem};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Runtime;

fn setup_cache() -> (CacheSystem, Runtime) {
    let rt = Runtime::new().unwrap_or_else(|_| panic!("Failed to create runtime"));
    let cache = rt.block_on(async {
        // SAFETY: This is the initialization of the test environment, so it is safe to set the
        // environment up.
        unsafe {
            std::env::set_var("REDIS_URL", "redis://127.0.0.1:6379");
        }
        CacheSystem::new()
            .await
            .unwrap_or_else(|_| panic!("Failed to create cache system"))
    });
    (cache, rt)
}

/// Benchmark stampede protection
fn bench_stampede_protection(c: &mut Criterion) {
    let (cache, rt) = setup_cache();
    let cache = Arc::new(cache);

    c.bench_function("stampede_100_concurrent", |b| {
        b.iter(|| {
            rt.block_on(async {
                let key = format!("bench:stampede:{}", rand::random::<u32>());
                let mut handles = vec![];

                for _ in 0..100 {
                    let cache = cache.clone();
                    let key = key.clone();
                    let handle = tokio::spawn(async move {
                        cache
                            .cache_manager()
                            .get_or_compute_with(&key, CacheStrategy::ShortTerm, || async {
                                tokio::time::sleep(Duration::from_millis(10)).await;
                                Ok(json!({"computed": true}))
                            })
                            .await
                            .unwrap_or_else(|_| panic!("Failed to compute"))
                    });
                    handles.push(handle);
                }

                for handle in handles {
                    black_box(handle.await.unwrap_or_else(|_| panic!("Task failed")));
                }
            });
        });
    });
}

criterion_group!(benches, bench_stampede_protection);
criterion_main!(benches);
