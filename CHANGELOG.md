# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Planned
- Integration tests with real Redis instance
- Benchmark suite with Criterion
- Graceful Redis reconnection
- Metrics export (Prometheus format)
- Cache invalidation patterns (wildcard, regex)

## [0.1.2] - 2025-01-03

### Changed
- **Documentation**: Significantly improved REDIS_URL configuration documentation
  - Added configuration priority order (programmatic > env var > .env > default)
  - Added use case examples (development, production, Docker, testing)
  - Added Redis URL format specification with examples
  - Added comprehensive troubleshooting section for common connection issues
  - Better organization with clear headings and code examples

### Internal
- No code changes - documentation-only release
- Fully backward compatible with 0.1.1 and 0.1.0

## [0.1.1] - 2025-01-03

### Changed
- **Documentation**: Removed RPS column from library comparison table in README
- **Documentation**: Removed unavailable docs.rs link from Contact section
- **Documentation**: Fixed GitHub repository URLs to use correct username

### Internal
- No code changes - documentation-only release
- Fully backward compatible with 0.1.0

## [0.1.0] - 2025-01-03

### Added

**Core Features:**
- Multi-tier caching architecture with L1 (Moka) and L2 (Redis)
- Cache stampede protection using DashMap + Mutex request coalescing
- Automatic L2-to-L1 promotion for frequently accessed data
- Comprehensive statistics tracking (hit rates, promotions, in-flight requests)

**Cache Strategies:**
- `RealTime` - 10 seconds TTL for fast-changing data
- `ShortTerm` - 5 minutes TTL for frequently accessed data
- `MediumTerm` - 1 hour TTL for moderately stable data
- `LongTerm` - 3 hours TTL for stable data
- `Custom(Duration)` - User-defined TTL

**Redis Streams Support:**
- `publish_to_stream()` - XADD with automatic trimming
- `read_stream_latest()` - XREVRANGE for latest N entries
- `read_stream()` - XREAD for blocking/non-blocking consumption

**API Methods:**
- `CacheSystem::new()` - Initialize with default Redis URL
- `CacheSystem::with_redis_url()` - Initialize with custom URL
- `CacheManager::get()` - Retrieve from cache (L1 → L2 fallback)
- `CacheManager::set_with_strategy()` - Store with TTL strategy
- `CacheManager::get_or_compute_with()` - Compute-on-miss with stampede protection
- `CacheManager::get_stats()` - Retrieve cache statistics

**Examples:**
- `basic_usage.rs` - Quick start and fundamental operations
- `stampede_protection.rs` - Demonstrates concurrency handling
- `redis_streams.rs` - Event streaming patterns
- `cache_strategies.rs` - All TTL strategies showcase
- `advanced_usage.rs` - L2-to-L1 promotion and compute-on-miss
- `health_monitoring.rs` - Health checks and statistics

**Documentation:**
- Comprehensive README.md with architecture diagrams
- Full rustdoc API documentation
- Migration guides from `cached` and `redis-rs`
- Performance benchmarks and comparison tables
- MIT OR Apache-2.0 dual licensing

### Performance

**Production Metrics** (from source project):
- **Throughput**: 16,829+ requests/second sustained
- **Latency**: 5.2ms average response time
- **Cache Hit Rate**: 95% overall (L1: 90%, L2: 75%)
- **Stampede Protection**: 99.6% latency reduction (534ms → 5.2ms in high-concurrency scenarios)
- **Success Rate**: 100% (zero failures under load)

**Resource Usage:**
- L1 Cache Capacity: 2,000 entries
- L2 Redis Connections: Multiplexed async connections
- Memory Footprint: ~50MB for typical workload
- CPU Overhead: <5% at 16k RPS

### Dependencies

- `moka = "0.12"` - L1 in-memory cache
- `redis = "0.32"` - L2 Redis client
- `tokio = "1.28"` - Async runtime
- `serde_json = "1.0"` - JSON serialization
- `anyhow = "1.0"` - Error handling
- `dashmap = "5.5"` - Concurrent HashMap for stampede protection

### Notes

This is the initial release extracted from a production web server project that serves a crypto investment dashboard. The library has been battle-tested at scale and proven reliable under high load.

The cache system was originally developed as `cache_system_island` module and has been refactored into a standalone, reusable library with zero business logic coupling.

[Unreleased]: https://github.com/yourusername/multi-tier-cache/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/yourusername/multi-tier-cache/releases/tag/v0.1.0
