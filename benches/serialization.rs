//! Benchmarks for serialization and type-safe caching

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use multi_tier_cache::{CacheStrategy, CacheSystem};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::Duration;
use tokio::runtime::Runtime;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct User {
    id: u64,
    name: String,
    email: String,
}

impl User {
    fn new(id: u64) -> Self {
        Self {
            id,
            name: format!("User {id}"),
            email: format!("user{id}@example.com"),
        }
    }
}

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

/// Benchmark JSON vs typed caching
fn bench_json_vs_typed(c: &mut Criterion) {
    let (cache, rt) = setup_cache();

    let mut group = c.benchmark_group("serialization");

    group.bench_function("json_cache", |b| {
        b.iter(|| {
            rt.block_on(async {
                let key = format!("bench:json:{}", rand::random::<u32>());
                let user = json!({
                    "id": 123,
                    "name": "Test User",
                    "email": "test@example.com"
                });

                cache
                    .cache_manager()
                    .set_with_strategy(&key, &user, CacheStrategy::ShortTerm)
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

    group.bench_function("typed_cache", |b| {
        b.iter(|| {
            rt.block_on(async {
                let key = format!("bench:typed:{}", rand::random::<u32>());
                let user = User::new(123);

                cache
                    .cache_manager()
                    .get_or_compute(&key, CacheStrategy::ShortTerm, || {
                        let u = user.clone();
                        async move { Ok(u) }
                    })
                    .await
                    .unwrap_or_else(|_| panic!("Failed to set cache"));

                black_box(
                    cache
                        .cache_manager()
                        .get_or_compute::<User, _, _>(&key, CacheStrategy::ShortTerm, || async {
                            panic!("Should not compute");
                        })
                        .await
                        .unwrap_or_else(|_| panic!("Failed to get cache")),
                );
            });
        });
    });

    group.finish();
}

/// Benchmark different data sizes
fn bench_data_sizes(c: &mut Criterion) {
    let (cache, rt) = setup_cache();

    let mut group = c.benchmark_group("data_size");
    group.measurement_time(Duration::from_secs(10));

    for size in &[100, 1024, 10240] {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            b.iter(|| {
                rt.block_on(async {
                    let key = format!("bench:size:{}", rand::random::<u32>());
                    let data = json!({"data": "x".repeat(size)});

                    cache
                        .cache_manager()
                        .set_with_strategy(&key, &data, CacheStrategy::ShortTerm)
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

criterion_group!(benches, bench_json_vs_typed, bench_data_sizes);
criterion_main!(benches);
