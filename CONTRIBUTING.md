# Contributing to multi-tier-cache

Thank you for your interest in contributing to multi-tier-cache! 🎉

## 🚀 Getting Started

### Fork and Clone

1. **Fork** the repository on GitHub
2. **Clone** your fork locally:
   ```bash
   git clone https://github.com/YOUR_USERNAME/multi-tier-cache.git
   cd multi-tier-cache
   ```

3. **Add upstream** remote:
   ```bash
   git remote add upstream https://github.com/ORIGINAL_OWNER/multi-tier-cache.git
   ```

### Development Environment

**Prerequisites:**
- Rust 1.70+ (2021 edition)
- Redis server running locally (for tests and examples)

**Install Redis** (Fedora):
```bash
sudo dnf install redis
sudo systemctl start redis
```

**Build the project:**
```bash
cargo build
```

**Run tests:**
```bash
cargo test
```

**Run examples:**
```bash
cargo run --example basic_usage
cargo run --example stampede_protection
cargo run --example redis_streams
```

## 💡 How to Contribute

### Reporting Bugs

Found a bug? Please create an issue with:

- **Title**: Clear, descriptive summary
- **Description**:
  - Expected behavior
  - Actual behavior
  - Steps to reproduce
  - Environment (OS, Rust version, Redis version)
  - Code snippet (if applicable)

**Example:**
```markdown
Title: Cache stampede protection not working with custom Duration

**Expected**: Only one request should compute value
**Actual**: Multiple requests compute simultaneously
**Steps**:
1. Use CacheStrategy::Custom(Duration::from_secs(30))
2. Fire 10 concurrent requests
3. Observe multiple computations in logs

**Environment**:
- OS: Fedora 42
- Rust: 1.75.0
- Redis: 7.2.0
```

### Suggesting Features

Have an idea? Open an issue with:

- **Use case**: Why is this feature needed?
- **Proposed API**: How would it work?
- **Alternatives**: Other ways to achieve the same goal?

### Pull Requests

#### Before You Start

1. **Check existing issues/PRs** to avoid duplicates
2. **Open an issue first** for major changes to discuss the approach
3. **Small PRs are better** - one feature/fix per PR

#### PR Process

1. **Create a branch:**
   ```bash
   git checkout -b feature/your-feature-name
   # or
   git checkout -b fix/your-bug-fix
   ```

2. **Make your changes:**
   - Write clean, documented code
   - Follow existing code style
   - Add tests for new functionality
   - Update documentation (README, rustdoc)

3. **Test your changes:**
   ```bash
   cargo test
   cargo clippy
   cargo fmt --check
   cargo doc --no-deps
   ```

4. **Commit with clear messages:**
   ```bash
   git commit -m "Add feature: cache invalidation patterns

   - Implement wildcard key matching
   - Add invalidate_pattern() method
   - Add tests and example
   "
   ```

5. **Push and create PR:**
   ```bash
   git push origin feature/your-feature-name
   ```
   Then open a PR on GitHub.

6. **PR description should include:**
   - What problem does this solve?
   - How does it work?
   - Any breaking changes?
   - Related issues (e.g., "Fixes #123")

## 📝 Code Style

### Rust Style

- **Run rustfmt** before committing:
  ```bash
  cargo fmt
  ```

- **Fix clippy warnings:**
  ```bash
  cargo clippy --all-targets --all-features -- -D warnings
  ```

- **Follow Rust API Guidelines**: https://rust-lang.github.io/api-guidelines/

### Documentation

- **Public APIs** must have rustdoc comments with examples:
  ```rust
  /// Stores a value in cache with specified strategy.
  ///
  /// # Arguments
  /// * `key` - Cache key
  /// * `value` - Data to cache
  /// * `strategy` - TTL strategy
  ///
  /// # Example
  /// ```rust
  /// use multi_tier_cache::{CacheSystem, CacheStrategy};
  ///
  /// # async fn example() -> anyhow::Result<()> {
  /// let cache = CacheSystem::new().await?;
  /// cache.cache_manager()
  ///     .set_with_strategy("key", value, CacheStrategy::ShortTerm)
  ///     .await?;
  /// # Ok(())
  /// # }
  /// ```
  pub async fn set_with_strategy(...) -> Result<()> {
      // ...
  }
  ```

- **Add examples** for new features in `examples/` directory

### Testing

- **Write tests** for new functionality:
  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;

      #[tokio::test]
      async fn test_cache_strategy_custom() {
          let cache = CacheSystem::new().await.unwrap();
          let strategy = CacheStrategy::Custom(Duration::from_secs(42));
          assert_eq!(strategy.to_duration().as_secs(), 42);
      }
  }
  ```

- **Integration tests** should go in `tests/` directory
- **Run all tests** before submitting PR:
  ```bash
  cargo test --all-features
  ```

## 🔍 Areas for Contribution

### High Priority

- [ ] Integration tests with real Redis instance
- [ ] Benchmark suite with Criterion
- [ ] Graceful Redis reconnection on connection loss
- [ ] Metrics export (Prometheus format)
- [ ] Cache invalidation patterns (wildcard, regex)

### Medium Priority

- [ ] Distributed cache invalidation
- [ ] Admin API for cache management
- [ ] Support for other backends (Memcached, etc.)
- [ ] Compression support for large values
- [ ] Async trait support for custom cache backends

### Documentation

- [ ] More examples (real-world use cases)
- [ ] Architecture deep-dive blog post
- [ ] Performance tuning guide
- [ ] Migration guide from other cache libraries

### Infrastructure

- [ ] GitHub Actions for benchmarking
- [ ] Automated changelog generation
- [ ] Security audit
- [ ] Fuzz testing

## 🐛 Debugging Tips

### Enable Debug Logging

```bash
# Run with debug output
RUST_LOG=debug cargo run --example basic_usage

# Verbose Redis commands
RUST_LOG=redis=trace cargo run --example basic_usage
```

### Redis Debugging

```bash
# Monitor Redis commands in real-time
redis-cli MONITOR

# Check cache keys
redis-cli KEYS "*"

# Inspect stream
redis-cli XINFO STREAM market_data_stream
```

### Performance Profiling

```bash
# Install flamegraph
cargo install flamegraph

# Generate flamegraph
cargo flamegraph --example stampede_protection
```

## 📋 Commit Message Convention

Follow [Conventional Commits](https://www.conventionalcommits.org/):

- `feat:` New feature
- `fix:` Bug fix
- `docs:` Documentation only
- `style:` Code style (formatting, etc.)
- `refactor:` Code refactoring
- `perf:` Performance improvement
- `test:` Adding/updating tests
- `chore:` Maintenance tasks

**Examples:**
```
feat: add cache invalidation pattern matching
fix: stampede protection not working with custom duration
docs: add migration guide from cached crate
perf: optimize L2-to-L1 promotion path
test: add integration tests for Redis Streams
```

## 🎯 Review Process

### What We Look For

- ✅ **Correctness**: Does it work as intended?
- ✅ **Tests**: Are edge cases covered?
- ✅ **Documentation**: Is it well-documented?
- ✅ **Performance**: Does it maintain high performance?
- ✅ **Safety**: Is it memory-safe and thread-safe?
- ✅ **Breaking Changes**: Are they justified and documented?

### Response Time

- We aim to respond to PRs within **48 hours**
- Complex PRs may take longer to review
- Feel free to ping maintainers after 1 week

## 🏆 Recognition

Contributors will be:
- Listed in CHANGELOG.md for their contributions
- Mentioned in release notes
- Added to GitHub contributors page automatically

## 📄 License

By contributing, you agree that your contributions will be licensed under:
- **MIT License** OR
- **Apache License 2.0**

Your choice (dual-licensed).

## 💬 Communication

- **Issues**: For bugs, features, questions
- **Discussions**: For general chat, ideas, Q&A (if enabled)
- **Pull Requests**: For code contributions

## 🙏 Thank You!

Every contribution matters, whether it's:
- Fixing a typo
- Improving documentation
- Adding a test
- Implementing a feature
- Reporting a bug

Thank you for helping make multi-tier-cache better! 🚀

---

**Questions?** Open an issue or start a discussion!
