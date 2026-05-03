# multi-tier-cache

[![Crates.io](https://img.shields.io/crates/v/multi-tier-cache.svg)](https://crates.io/crates/multi-tier-cache)
[![docs.rs](https://docs.rs/multi-tier-cache/badge.svg)](https://docs.rs/multi-tier-cache)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](LICENSE-MIT)

A high-performance, production-ready multi-tier caching library for Rust.

Combine blazing-fast in-memory caching (Moka / QuickCache) with persistent distributed storage (Redis / Memcached) in a single, unified API — with stampede protection, cross-instance invalidation, and dynamic N-tier architectures out of the box.

```
Request ─→ L1 (RAM) ─→ L2 (Redis) ─→ L3 (Cold) ─→ … ─→ Compute
            <1ms          2-5ms         10-50ms            your latency
```

## Highlights

- **Multi-tier by default** — L1 + L2 out of the box, extensible to L3, L4, … LN with per-tier TTL scaling
- **Stampede protection** — broadcast-channel request coalescing prevents thundering herds (99.6% latency reduction)
- **Cross-instance invalidation** — real-time cache sync across servers via Redis Pub/Sub
- **Pluggable backends** — swap Moka, Redis, DashMap, QuickCache, Memcached, or bring your own
- **Type-safe compute-on-miss** — `get_or_compute_typed::<T>()` handles serialization automatically
- **Redis Streams** — built-in pub/sub with automatic trimming for event-driven architectures
- **Zero-cost L1 hits** — raw `bytes::Bytes` storage, no intermediate JSON AST allocations
- **Probabilistic promotion** — configurable 1/N promotion frequency to avoid L1 pollution
- **Production-proven** — battle-tested at 21,528+ RPS with 19.0ms p50 latency and 95% hit rate

## Table of Contents

- [Quick Start](#quick-start)
- [Feature Flags](#feature-flags)
- [Cache Strategies](#cache-strategies)
- [Compute-on-Miss](#compute-on-miss)
- [Type-Safe Caching](#type-safe-caching)
- [Cross-Instance Invalidation](#cross-instance-invalidation)
- [Multi-Tier Architecture](#multi-tier-architecture)
- [Available Backends](#available-backends)
- [Custom Backends](#custom-backends)
- [Redis Streams](#redis-streams)
- [Error Handling](#error-handling)
- [Configuration](#configuration)
- [Performance](#performance)
- [Testing](#testing)
- [Examples](#examples)
- [Migration Guide](#migration-guide)
- [Contributing](#contributing)
- [License](#license)

## Quick Start

Add to your `Cargo.toml`:

```toml
[dependencies]
multi-tier-cache = "0.6"
tokio = { version = "1", features = ["full"] }
```

```rust
use multi_tier_cache::{CacheSystem, CacheStrategy};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cache = CacheSystem::new().await?;

    // Store with a predefined TTL strategy
    let data = bytes::Bytes::from(r#"{"user":"alice","score":100}"#);
    cache.cache_manager()
        .set_with_strategy("user:1", data, CacheStrategy::ShortTerm)
        .await?;

    // Retrieve — checks L1 first, falls back to L2
    if let Some(value) = cache.cache_manager().get("user:1").await? {
        println!("cached: {:?}", value);
    }

    // Inspect hit rates
    let stats = cache.cache_manager().get_stats();
    println!("hit rate: {:.1}%", stats.hit_rate);

    Ok(())
}
```

> **Note:** By default the library connects to Redis at `redis://127.0.0.1:6379`. Set the `REDIS_URL` environment variable or use `CacheSystem::with_redis_url()` to override. See [Configuration](#configuration) for details.

## Feature Flags

All backends are optional and feature-gated to keep the dependency tree lean.

| Feature | Description | Default |
|---------|-------------|:-------:|
| `moka` | Moka in-memory cache (L1) | ✅ |
| `redis` | Redis distributed cache (L2) | ✅ |
| `backend-quickcache` | QuickCache ultra-fast L1 backend | — |
| `backend-memcached` | Memcached distributed L2 backend | — |
| `bincode` | Bincode binary serialization | — |
| `msgpack` | MessagePack serialization | — |
| `full` | Enable everything above | — |

```toml
# Default — Moka L1 + Redis L2
multi-tier-cache = "0.6"

# In-memory only (no Redis dependency)
multi-tier-cache = { version = "0.6", default-features = false, features = ["moka"] }

# All backends + all serializers
multi-tier-cache = { version = "0.6", features = ["full"] }
```

## Cache Strategies

Built-in TTL presets for common use cases:

```rust
use multi_tier_cache::CacheStrategy;
use std::time::Duration;

// Predefined strategies
CacheStrategy::RealTime    // 10 seconds — live prices, counters
CacheStrategy::ShortTerm   //  5 minutes — sessions, hot data
CacheStrategy::MediumTerm  //  1 hour    — catalogs, API responses
CacheStrategy::LongTerm    //  3 hours   — config, stable data

// Or specify your own
CacheStrategy::Custom(Duration::from_secs(30))
```

```rust
cache.cache_manager()
    .set_with_strategy("live:price", data, CacheStrategy::RealTime)
    .await?;
```

## Compute-on-Miss

Fetch data only on cache miss. Concurrent requests for the same key are coalesced — only **one** computation runs, the rest wait and share the result.

```rust
let value = cache.cache_manager()
    .get_or_compute_with(
        "product:42",
        CacheStrategy::MediumTerm,
        || async { fetch_from_database(42).await },
    )
    .await?;
```

## Type-Safe Caching

Skip manual serialization entirely. `get_or_compute_typed` handles `Serialize`/`DeserializeOwned` for you:

```rust
use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize)]
struct User {
    id: i64,
    name: String,
    email: String,
}

let user: User = cache.cache_manager()
    .get_or_compute_typed(
        "user:123",
        CacheStrategy::MediumTerm,
        || async {
            sqlx::query_as::<_, User>("SELECT * FROM users WHERE id = $1")
                .bind(123)
                .fetch_one(&pool)
                .await
        },
    )
    .await?;
```

Works with any `T: Serialize + DeserializeOwned` — database rows, API responses, computed analytics, etc.

## Cross-Instance Invalidation

In distributed deployments, L1 caches on different servers can become stale. The invalidation system uses Redis Pub/Sub to synchronize all instances in real time.

### Setup

```rust
use multi_tier_cache::{CacheManager, L1Cache, L2Cache, InvalidationConfig};
use std::sync::Arc;

let cache_manager = CacheManager::new_with_invalidation(
    Arc::new(L1Cache::new(Default::default())?),
    Arc::new(L2Cache::new().await?),
    "redis://localhost",
    InvalidationConfig::default(),
).await?;
```

### Strategies

**Remove** — evict and reload lazily on next access:

```rust
database.update_user(123, &new_data).await?;
cache_manager.invalidate("user:123").await?;
```

**Update** — push new data to all instances (zero cache miss):

```rust
let bytes = Bytes::from(serde_json::to_vec(&new_data)?);
cache_manager.update_cache("user:123", bytes, Some(Duration::from_secs(3600))).await?;
```

**Pattern** — invalidate multiple related keys at once:

```rust
cache_manager.invalidate_pattern("product:category:42:*").await?;
```

**Write-through** — cache locally and broadcast in one call:

```rust
cache_manager.set_with_broadcast("report:monthly", bytes, CacheStrategy::LongTerm).await?;
```

### Propagation flow

```
Instance A              Redis Pub/Sub           Instance B
    │  update data          │                       │
    │  broadcast  ─────────>│                       │
    │                       │  deliver  ──────────>│
    │                       │                  update L1
```

| Strategy | Bandwidth | Cache Miss on Next Read | Best For |
|----------|:---------:|:-----------------------:|----------|
| Remove | Low | Yes | Large values, infrequent reads |
| Update | Higher | No | Small values, frequent reads |
| Pattern | Medium | Yes | Bulk invalidation |

### Configuration

```rust
let config = InvalidationConfig {
    channel: "my_app:cache:invalidate".to_string(),
    auto_broadcast_on_write: false,
    enable_audit_stream: true,
    audit_stream: "cache:invalidations".to_string(),
    audit_stream_maxlen: Some(10_000),
};
```

## Multi-Tier Architecture

Go beyond L1 + L2. Add L3 (cold storage), L4 (archive), or any number of tiers — each with its own TTL multiplier and promotion policy.

```
L1 (RAM)  →  L2 (Redis)  →  L3 (RocksDB)  →  L4 (S3)
 <1ms          2-5ms          10-50ms          100-500ms
 95% hits       4%             0.9%             0.1%
```

### Building a 3-tier cache

```rust
use multi_tier_cache::{CacheSystemBuilder, TierConfig};

let cache = CacheSystemBuilder::new()
    .with_tier(moka_l1, TierConfig::as_l1())
    .with_tier(redis_l2, TierConfig::as_l2())
    .with_l3(rocksdb_l3)   // convenience: 2× TTL
    .build()
    .await?;
```

### Tier presets

| Preset | Promotion | TTL Scale | Purpose |
|--------|:---------:|:---------:|---------|
| `TierConfig::as_l1()` | — | 1× | Hot data in RAM |
| `TierConfig::as_l2()` | → L1 | 1× | Warm distributed |
| `TierConfig::as_l3()` | → L2 → L1 | 2× | Cold storage |
| `TierConfig::as_l4()` | → all | 8× | Archive |

Custom tiers:

```rust
TierConfig::new(3)
    .with_promotion(true)
    .with_ttl_scale(5.0)
    .with_promotion_frequency(20) // promote 1 in 20 hits
```

### TTL scaling example

Setting `CacheStrategy::MediumTerm` (1 hour):

| Tier | Scale | Effective TTL |
|------|:-----:|:-------------:|
| L1 | 1× | 1 h |
| L2 | 1× | 1 h |
| L3 | 2× | 2 h |
| L4 | 8× | 8 h |

### Automatic promotion

When data is found in a lower tier, it is promoted upward automatically:

```
GET "key"
 ├─ L1 → miss
 ├─ L2 → miss
 └─ L3 → HIT → promote to L2 → promote to L1 → return
```

### Per-tier statistics

```rust
if let Some(tier_stats) = cache.cache_manager().get_tier_stats() {
    for s in tier_stats {
        println!("L{}: {} hits ({})", s.tier_level, s.hit_count(), s.backend_name);
    }
}
```

### Backward compatibility

Existing 2-tier code works without changes. Multi-tier mode is opt-in via `.with_tier()`.

```rust
// Still works exactly as before
let cache = CacheSystemBuilder::new().build().await?;
```

## Available Backends

### In-Memory (L1)

| Backend | Feature | Eviction | Notes |
|---------|---------|----------|-------|
| **MokaCache** | `moka` *(default)* | Automatic (LRU + TTL) | Production recommended |
| **DashMapCache** | *always available* | Manual cleanup | Simple, no eviction policy |
| **QuickCacheBackend** | `backend-quickcache` | Automatic (LRU) | Maximum throughput |

### Distributed (L2)

| Backend | Feature | Persistence | TTL Introspection |
|---------|---------|:-----------:|:-----------------:|
| **RedisCache** | `redis` *(default)* | Yes | ✅ |
| **MemcachedCache** | `backend-memcached` | No | ❌ |

### Usage

```rust
use multi_tier_cache::{CacheSystemBuilder, CacheBackend, DashMapCache};
use std::sync::Arc;

let cache = CacheSystemBuilder::new()
    .with_l1(Arc::new(DashMapCache::new()) as Arc<dyn CacheBackend>)
    .build()
    .await?;
```

QuickCache (requires feature flag):

```toml
multi-tier-cache = { version = "0.6", features = ["backend-quickcache"] }
```

```rust
use multi_tier_cache::{QuickCacheBackend, CacheSystemBuilder, CacheBackend};

let cache = CacheSystemBuilder::new()
    .with_l1(Arc::new(QuickCacheBackend::new(5000).await?) as Arc<dyn CacheBackend>)
    .build()
    .await?;
```

## Custom Backends

Implement `CacheBackend` for L1, or `L2CacheBackend` (extends `CacheBackend` with `get_with_ttl`) for L2.

### L1 example

```rust
use multi_tier_cache::{CacheBackend, CacheError, CacheResult};
use bytes::Bytes;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

struct HashMapCache {
    store: Arc<RwLock<HashMap<String, (Bytes, Instant)>>>,
}

impl CacheBackend for HashMapCache {
    async fn get(&self, key: &str) -> Option<Bytes> {
        let store = self.store.read().unwrap();
        store.get(key).and_then(|(v, exp)| {
            (*exp > Instant::now()).then(|| v.clone())
        })
    }

    async fn set_with_ttl(&self, key: &str, value: Bytes, ttl: Duration) -> CacheResult<()> {
        self.store.write().unwrap()
            .insert(key.to_string(), (value, Instant::now() + ttl));
        Ok(())
    }

    async fn remove(&self, key: &str) -> CacheResult<()> {
        self.store.write().unwrap().remove(key);
        Ok(())
    }

    async fn health_check(&self) -> bool { true }
    fn name(&self) -> &'static str { "HashMap" }
}
```

### L2 example

```rust
use multi_tier_cache::L2CacheBackend;

impl L2CacheBackend for MyDistributedCache {
    async fn get_with_ttl(&self, key: &str) -> Option<(Bytes, Option<Duration>)> {
        // Return value with remaining TTL for accurate promotion
        Some((value, Some(remaining_ttl)))
    }
}
```

### Builder API

```rust
let cache = CacheSystemBuilder::new()
    .with_l1(custom_l1)         // any Arc<dyn CacheBackend>
    .with_l2(custom_l2)         // any Arc<dyn L2CacheBackend>
    .with_streams(custom_stream) // optional Arc<dyn StreamingBackend>
    .build()
    .await?;
```

See [`examples/custom_backends.rs`](examples/custom_backends.rs) for complete working examples.

## Redis Streams

Built-in publish/subscribe with automatic trimming:

```rust
// Publish an event
let fields = vec![
    ("event_type".into(), "user_signup".into()),
    ("user_id".into(), "42".into()),
];
cache.cache_manager()
    .publish_to_stream("events", fields, Some(1000))
    .await?;

// Read latest entries
let entries = cache.cache_manager()
    .read_stream_latest("events", 10)
    .await?;

// Blocking read for new entries (5s timeout)
let new = cache.cache_manager()
    .read_stream("events", "$", 10, Some(5000))
    .await?;
```

## Error Handling

All operations return `CacheResult<T>`, powered by a structured `CacheError` enum:

```rust
use multi_tier_cache::CacheError;

match cache.cache_manager().get("key").await {
    Ok(Some(value)) => { /* cache hit */ }
    Ok(None)        => { /* cache miss */ }
    Err(CacheError::BackendError(msg))       => eprintln!("backend: {msg}"),
    Err(CacheError::SerializationError(msg)) => eprintln!("serde: {msg}"),
    Err(CacheError::InvalidationError(msg))  => eprintln!("invalidation: {msg}"),
    Err(CacheError::ConfigError(msg))        => eprintln!("config: {msg}"),
    Err(CacheError::NotFound)                => eprintln!("not found"),
    Err(CacheError::InternalError(msg))      => eprintln!("internal: {msg}"),
}
```

`CacheError` implements `From` for `RedisError`, `serde_json::Error`, `MemcacheError`, and more — so `?` works seamlessly.

## Configuration

### Redis Connection

| Priority | Method |
|----------|--------|
| 1 | `CacheSystem::with_redis_url("redis://…")` |
| 2 | `REDIS_URL` environment variable |
| 3 | `.env` file |
| 4 | Default: `redis://127.0.0.1:6379` |

```bash
# Shell
export REDIS_URL="redis://:password@redis-host:6379"

# .env file
REDIS_URL="redis://localhost:6379"
```

Redis URL format:

```
redis://[username]:[password]@[host]:[port]/[database]
rediss://…   # TLS connection
```

### Moka L1 Configuration

```rust
use multi_tier_cache::{CacheSystemBuilder, MokaCacheConfig};
use std::time::Duration;

let config = MokaCacheConfig {
    max_capacity: 10_000,
    time_to_live: Duration::from_secs(30 * 60),
    time_to_idle: Duration::from_secs(5 * 60),
};

let cache = CacheSystemBuilder::new()
    .with_moka_config(config)
    .build()
    .await?;
```

### Default Tuning

| Parameter | Default |
|-----------|---------|
| L1 capacity | 2 000 entries |
| L1 TTL | 5 min (per key) |
| L2 TTL | 1 h (per key) |
| Stream max length | 1 000 entries |

### Docker Compose

```yaml
services:
  app:
    environment:
      - REDIS_URL=redis://redis:6379
  redis:
    image: redis:7-alpine
    ports:
      - "6379:6379"
```

## Performance

Benchmarked in production (Google Cloud e2-micro):

| Metric | Value |
|--------|------:|
| Throughput | 21,528+ req/s |
| Latency (p50) | 19.0 ms |
| Latency (mean) | 23.2 ms |
| Cache hit rate | 95%+ |
| Stampede reduction | 99.6% |
| Error rate | 0% (50k requests) |

### Comparison

| Library | Multi-Tier | Stampede Protection | Redis | Streams | Invalidation |
|---------|:----------:|:-------------------:|:-----:|:-------:|:------------:|
| **multi-tier-cache** | ✅ N-tier | ✅ broadcast | ✅ | ✅ | ✅ Pub/Sub |
| cached | — | — | — | — | — |
| moka | L1 only | L1 only | — | — | — |
| redis-rs | — | manual | ✅ | manual | manual |

### Running Benchmarks

```bash
cargo bench                            # all suites
cargo bench --bench cache_operations   # L1/L2 read/write
cargo bench --bench stampede_protection
cargo bench --bench invalidation
cargo bench --bench serialization
cargo bench --bench multi_tier
cargo bench --bench storm_requests     # concurrent stress
```

HTML reports are saved to `target/criterion/`.

## Testing

Integration tests run against a real Redis instance:

```bash
cargo test --tests                        # all tests
cargo test --test integration_basic       # core operations
cargo test --test integration_invalidation
cargo test --test integration_stampede
cargo test --test integration_streams
```

**Requirements:** Redis on `localhost:6379` (or set `REDIS_URL`). Tests clean up after themselves.

**Coverage:** L1/L2 get/set/remove, TTL, promotion, stampede protection, cross-instance invalidation (remove, update, pattern), type-safe caching, Redis Streams, statistics.

## Examples

```bash
cargo run --example basic_usage
cargo run --example cache_strategies
cargo run --example stampede_protection
cargo run --example database_caching
cargo run --example redis_streams
cargo run --example advanced_usage
cargo run --example health_monitoring
cargo run --example custom_backends
cargo run --example builtin_backends
cargo run --example multi_tier_usage
cargo run --example probabilistic_promotion
cargo run --example tracing_demo
```

## Migration Guide

### From `cached`

```rust
// Before (cached)
#[cached(time = 60)]
fn expensive(arg: String) -> String { /* … */ }

// After (multi-tier-cache)
let result = cache.cache_manager()
    .get_or_compute_with(
        &format!("fn:{arg}"),
        CacheStrategy::ShortTerm,
        || async { compute(arg).await },
    )
    .await?;
```

### From `redis-rs`

```rust
// Before
let value: String = conn.get("key")?;
conn.set_ex("key", value, 3600)?;

// After
let value = cache.cache_manager().get("key").await?;
cache.cache_manager()
    .set_with_strategy("key", data, CacheStrategy::MediumTerm)
    .await?;
```

## Feature Compatibility

| Feature | Default Redis L2 | Custom L2 Backend |
|---------|:-----------------:|:-----------------:|
| Single-key invalidation | ✅ | ✅ |
| Pattern invalidation | ✅ | ⚠️ not available |
| Type-safe caching | ✅ | ✅ |
| Stampede protection | ✅ | ✅ |
| Streaming | ✅ | requires `StreamingBackend` |

> **Note:** Pattern-based invalidation (`invalidate_pattern`) requires the concrete `RedisCache` L2 backend for `SCAN` support.

## Contributing

Contributions are welcome! Please feel free to submit a Pull Request. See [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.

## License

Licensed under either of:

- [MIT License](LICENSE-MIT)
- [Apache License 2.0](LICENSE-APACHE)

at your option.

## Acknowledgments

Built on top of [Moka](https://github.com/moka-rs/moka), [redis-rs](https://github.com/redis-rs/redis-rs), [DashMap](https://github.com/xacrimon/dashmap), and [Tokio](https://tokio.rs).

---

<sub>Made with ❤️ in Rust · Production-proven in crypto trading at 21,528+ RPS</sub>
