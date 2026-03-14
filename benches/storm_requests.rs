use criterion::{criterion_group, criterion_main, Criterion};
use multi_tier_cache::{CacheSystem, CacheStrategy, CacheBackend, L2CacheBackend, TierConfig, CacheSystemBuilder, L1Cache, L2Cache, CacheManager, MokaCacheConfig};
use tokio::runtime::Runtime;
use bytes::Bytes;
use reqwest;
use std::sync::Arc;
use std::time::Duration;
use futures_util::future::join_all;

async fn setup_bench() -> (CacheSystem, Vec<String>, usize) {
    println!("Initializing Cache System...");
    let cache = CacheSystem::new().await.expect("Failed to init cache");
    
    let url = "https://thichuong.github.io/ambidex_survival/";
    println!("Fetching content from {}...", url);
    let content = reqwest::get(url).await.expect("Failed to fetch URL")
        .bytes().await.expect("Failed to read bytes");
    
    let size_bytes = content.len();
    println!("Fetched content size: {} bytes ({:.2} KB)", size_bytes, size_bytes as f64 / 1024.0);
    
    let content_str = String::from_utf8_lossy(&content).to_string();
    
    println!("Pre-populating 10,000 unique keys (keys are clones/variants of the web content)...");
    let mut keys = Vec::with_capacity(10000);
    for i in 0..10000 {
        // Create unique content by appending index to the original HTML
        let unique_content = format!("{}_{}", content_str, i);
        let key = unique_content.clone();
        let val = Bytes::from(unique_content);
        
        cache.cache_manager()
            .set_with_strategy(&key, val, CacheStrategy::LongTerm)
            .await
            .expect("Failed to set key");
        
        keys.push(key);
        
        if (i + 1) % 1000 == 0 {
            println!("  Populated {} keys...", i + 1);
        }
    }
    
    (cache, keys, size_bytes)
}

async fn setup_l2_only_bench() -> (Arc<CacheManager>, Vec<String>, usize) {
    println!("Initializing L2-only Cache System (No Promotion)...");
    
    let l1 = Arc::new(L1Cache::new(MokaCacheConfig::default()).expect("Failed to init L1"));
    let l2 = Arc::new(L2Cache::new().await.expect("Failed to init L2"));

    // Builder with promotion disabled for L2
    let cache_system = CacheSystemBuilder::new()
        .with_tier(Arc::clone(&l1) as Arc<dyn L2CacheBackend>, TierConfig::as_l1())
        .with_tier(Arc::clone(&l2) as Arc<dyn L2CacheBackend>, TierConfig::as_l2().with_promotion(false))
        .build()
        .await
        .expect("Failed to build cache system");
    
    let manager = cache_system.cache_manager;

    let url = "https://thichuong.github.io/ambidex_survival/";
    let content = reqwest::get(url).await.expect("Failed to fetch URL")
        .bytes().await.expect("Failed to read bytes");
    
    let size_bytes = content.len();
    let content_str = String::from_utf8_lossy(&content).to_string();
    
    println!("Pre-populating 10,000 unique keys to L2...");
    let mut keys = Vec::with_capacity(10000);
    for i in 0..10000 {
        let unique_content = format!("{}_{}", content_str, i);
        let key = unique_content.clone();
        let val = Bytes::from(unique_content);
        
        // Set directly in L2 backend to ensure L1 starts empty
        l2.set_with_ttl(&key, val, Duration::from_secs(10800)).await.expect("Failed to set L2");
        keys.push(key);
    }
    
    (manager, keys, size_bytes)
}

fn storm_requests_bench(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let (cache, keys, size) = rt.block_on(setup_bench());
    
    let single_key = keys[0].clone();
    let size_kb = size as f64 / 1024.0;

    let mut group = c.benchmark_group(format!("Storm Requests (Size: {:.1} KB)", size_kb));
    // Report throughput as RPS (1 iteration = 10,000 requests)
    group.throughput(criterion::Throughput::Elements(10000));
    // Reduce sample size because each iteration performs 10,000 cache operations
    group.sample_size(10);
    
    // Scenario 1: 10,000 requests to 1 type (single key)
    group.bench_function("Scenario 1: 10,000 single key requests", |b| {
        b.to_async(&rt).iter(|| async {
            let futures = (0..10000).map(|_| cache.cache_manager().get(&single_key));
            let _results = join_all(futures).await;
        });
    });

    // Scenario 2: 10,000 requests to 10,000 different keys
    group.bench_function("Scenario 2: 10,000 unique key requests", |b| {
        b.to_async(&rt).iter(|| async {
            let futures = keys.iter().map(|key| cache.cache_manager().get(key));
            let _results = join_all(futures).await;
        });
    });

    group.finish();

    // Scenario 3: L2-only (No Promotion)
    let (cache_l2, keys_l2, size_l2) = rt.block_on(setup_l2_only_bench());
    let size_kb_l2 = size_l2 as f64 / 1024.0;
    let mut group_l2 = c.benchmark_group(format!("L2-Only Storm (No Promotion, Size: {:.1} KB)", size_kb_l2));
    group_l2.throughput(criterion::Throughput::Elements(10000));
    group_l2.sample_size(10);

    group_l2.bench_function("Scenario 3: 10,000 unique key requests from L2 ONLY", |b| {
        b.to_async(&rt).iter(|| async {
            let futures = keys_l2.iter().map(|key| cache_l2.get(key));
            let _results = join_all(futures).await;
        });
    });

    group_l2.finish();
}

criterion_group!(benches, storm_requests_bench);
criterion_main!(benches);
