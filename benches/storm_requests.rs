use criterion::{criterion_group, criterion_main, Criterion};
use multi_tier_cache::{CacheSystem, CacheStrategy};
use tokio::runtime::Runtime;
use bytes::Bytes;
use reqwest;

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
            for _ in 0..10000 {
                let result = cache.cache_manager().get(&single_key).await.unwrap();
                assert!(result.is_some());
            }
        });
    });

    // Scenario 2: 10,000 requests to 10,000 different keys
    group.bench_function("Scenario 2: 10,000 unique key requests", |b| {
        b.to_async(&rt).iter(|| async {
            for key in &keys {
                let result = cache.cache_manager().get(key).await.unwrap();
                assert!(result.is_some());
            }
        });
    });

    group.finish();
}

criterion_group!(benches, storm_requests_bench);
criterion_main!(benches);
