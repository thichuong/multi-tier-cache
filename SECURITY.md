# Security Policy

## Supported Versions

We release patches for security vulnerabilities in the following versions:

| Version | Supported          |
| ------- | ------------------ |
| 0.1.x   | :white_check_mark: |

## Reporting a Vulnerability

**Please DO NOT report security vulnerabilities through public GitHub issues.**

If you discover a security vulnerability in multi-tier-cache, please send an email to:

📧 **your.email@example.com** (replace with actual email)

### What to Include

Please include as much of the following information as possible:

- **Type of vulnerability** (e.g., buffer overflow, SQL injection, cross-site scripting, etc.)
- **Full paths** of source file(s) related to the vulnerability
- **Location** of the affected source code (tag/branch/commit or direct URL)
- **Step-by-step instructions** to reproduce the issue
- **Proof-of-concept or exploit code** (if possible)
- **Impact** of the vulnerability, including how an attacker might exploit it

### Response Timeline

- **Initial Response**: Within **48 hours** of report
- **Confirmation**: Within **7 days** (confirm if it's a real vulnerability)
- **Fix Timeline**: Within **30 days** for critical issues, **90 days** for non-critical
- **Disclosure**: Coordinated disclosure after fix is released

### What to Expect

1. **Acknowledgment**: We will acknowledge your email within 48 hours
2. **Investigation**: We will investigate and confirm the vulnerability
3. **Fix Development**: We will develop and test a fix
4. **Coordinated Disclosure**: We will coordinate with you on timing
5. **Credit**: You will be credited in the security advisory (if desired)

## Security Best Practices

### For Library Users

When using multi-tier-cache in your applications:

#### 1. Secure Redis Configuration

```bash
# ❌ BAD: Plain Redis without authentication
export REDIS_URL="redis://localhost:6379"

# ✅ GOOD: Redis with authentication
export REDIS_URL="redis://:password@localhost:6379"

# ✅ BETTER: Redis with TLS
export REDIS_URL="rediss://username:password@redis.example.com:6380"
```

#### 2. Environment Variables

```rust
// ❌ BAD: Hardcoded credentials
let cache = CacheSystem::with_redis_url("redis://:secret@localhost:6379").await?;

// ✅ GOOD: Use environment variables
let redis_url = std::env::var("REDIS_URL").expect("REDIS_URL must be set");
let cache = CacheSystem::with_redis_url(&redis_url).await?;

// ✅ BETTER: Use environment variables with validation
use std::env;
let redis_url = env::var("REDIS_URL")
    .unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());
let cache = CacheSystem::with_redis_url(&redis_url).await?;
```

#### 3. Data Sanitization

```rust
// ⚠️  WARNING: Validate cache keys
fn sanitize_cache_key(user_input: &str) -> String {
    // Prevent cache key injection
    user_input
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == ':' || *c == '_' || *c == '-')
        .collect()
}

let key = sanitize_cache_key(&user_input);
cache.cache_manager().get(&key).await?;
```

#### 4. Sensitive Data

```rust
// ⚠️  WARNING: Don't cache sensitive data without encryption
let user_password = "secret123"; // ❌ DON'T DO THIS

// ✅ If you must cache sensitive data, encrypt it first
use aes_gcm::Aes256Gcm; // Example encryption
let encrypted_data = encrypt_sensitive_data(&sensitive_data)?;
cache.cache_manager()
    .set_with_strategy("user:123:encrypted", encrypted_data, CacheStrategy::ShortTerm)
    .await?;
```

#### 5. TTL Considerations

```rust
// ⚠️  WARNING: Don't use LongTerm for data that changes frequently
// This can lead to stale data vulnerabilities

// ✅ GOOD: Match TTL to data sensitivity
cache.cache_manager()
    .set_with_strategy(
        "user:session",
        session_data,
        CacheStrategy::ShortTerm // 5 minutes for session data
    )
    .await?;
```

### Redis Security Checklist

When deploying Redis for production use:

- [ ] **Enable authentication**: Set `requirepass` in redis.conf
- [ ] **Use TLS/SSL**: Enable `tls-port` and configure certificates
- [ ] **Bind to localhost**: Set `bind 127.0.0.1` unless networked
- [ ] **Disable dangerous commands**:
  ```
  rename-command FLUSHDB ""
  rename-command FLUSHALL ""
  rename-command CONFIG ""
  ```
- [ ] **Enable persistence**: Configure RDB/AOF for data durability
- [ ] **Set maxmemory**: Prevent OOM with `maxmemory` policy
- [ ] **Use Redis ACL**: Fine-grained access control (Redis 6+)
- [ ] **Regular updates**: Keep Redis version up-to-date
- [ ] **Firewall rules**: Restrict access to Redis port (6379)
- [ ] **Monitor logs**: Watch for suspicious activity

## Known Security Considerations

### 1. Cache Stampede Mitigation

**Risk**: Cache stampede can cause DDoS-like behavior on backend services.

**Mitigation**: Built-in stampede protection using DashMap + Mutex coalescing.

**Note**: Stampede protection is enabled by default and requires no configuration.

### 2. Redis Connection Security

**Risk**: Unencrypted Redis connections can expose cached data.

**Mitigation**: Use `rediss://` (TLS) protocol in production:
```bash
export REDIS_URL="rediss://username:password@redis.example.com:6380"
```

### 3. Memory Exhaustion

**Risk**: Unbounded cache growth can lead to OOM.

**Mitigation**:
- L1 Cache: Limited to 2,000 entries (configurable in source)
- L2 Cache: Configure Redis `maxmemory` and eviction policy
- Redis Streams: Auto-trimming with `MAXLEN` (default: 1000 entries)

### 4. Data Persistence

**Risk**: Redis data loss on restart.

**Mitigation**: Configure Redis persistence (RDB or AOF) based on durability requirements.

**Note**: L1 cache is in-memory only and will be lost on restart.

## Dependency Security

We monitor dependencies for security vulnerabilities using:

- **GitHub Dependabot**: Automated dependency updates
- **cargo-audit**: Security audits for Rust dependencies

### Running Security Audit

```bash
# Install cargo-audit
cargo install cargo-audit

# Run security audit
cargo audit

# Auto-fix known vulnerabilities
cargo audit fix
```

### Dependency Update Policy

- **Critical vulnerabilities**: Patched within 48 hours
- **High vulnerabilities**: Patched within 7 days
- **Medium/Low vulnerabilities**: Patched in next minor release

## Security Updates

Security updates will be:

1. **Published** as GitHub Security Advisories
2. **Released** as patch versions (e.g., 0.1.1)
3. **Announced** in CHANGELOG.md
4. **Communicated** via GitHub release notes

### Subscribing to Updates

- **Watch** this repository on GitHub
- **Enable notifications** for security advisories
- **Subscribe** to releases (RSS feed available)

## Security Testing

We perform:

- **Static analysis**: Using `cargo clippy` with security lints
- **Dependency audits**: Using `cargo audit` in CI/CD
- **Fuzz testing**: (Planned for future releases)
- **Penetration testing**: (Planned for major releases)

## Responsible Disclosure

We practice responsible disclosure:

- **90-day disclosure window**: For non-critical issues
- **Immediate disclosure**: For actively exploited vulnerabilities
- **Coordinated disclosure**: With affected parties
- **Public advisory**: After fix is released

## Hall of Fame

Security researchers who have helped improve multi-tier-cache:

<!-- Will be populated as vulnerabilities are reported and fixed -->

*No security reports yet. Be the first to help improve our security!*

## Legal

This security policy is provided as-is without warranty. By reporting vulnerabilities, you agree to:

- Not publicly disclose the vulnerability before coordinated release
- Not exploit the vulnerability maliciously
- Comply with all applicable laws

## References

- [OWASP Security Cheat Sheet](https://cheatsheetseries.owasp.org/)
- [Redis Security](https://redis.io/docs/management/security/)
- [Rust Security Guidelines](https://anssi-fr.github.io/rust-guide/)

---

**Last Updated**: 2025-01-03

For questions about this policy, open an issue or contact: your.email@example.com
