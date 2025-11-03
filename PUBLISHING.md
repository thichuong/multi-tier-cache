# Publishing Guide: multi-tier-cache

Complete step-by-step guide to publish this library to GitHub and crates.io.

---

## 📋 Pre-Publishing Checklist

Before publishing, ensure:

- [x] Code builds without warnings (`cargo build --release`)
- [x] All tests pass (`cargo test`)
- [x] Examples run successfully (`cargo run --example basic_usage`)
- [x] Documentation is complete (`cargo doc --open`)
- [x] README.md is comprehensive
- [x] LICENSE files exist (MIT + Apache-2.0)
- [x] Cargo.toml metadata is filled
- [ ] Update author information in Cargo.toml
- [ ] Choose a GitHub repository name
- [ ] Create crates.io account (if publishing)

---

## 🔧 Step 1: Update Cargo.toml Metadata

Edit `/home/thichuong/Desktop/multi-tier-cache/Cargo.toml`:

```toml
[package]
name = "multi-tier-cache"
version = "0.1.0"
edition = "2021"
authors = ["Your Name <your.email@example.com>"]  # ← UPDATE THIS
description = "High-performance multi-tier cache with L1 (Moka) + L2 (Redis) and stampede protection"
license = "MIT OR Apache-2.0"
repository = "https://github.com/YOURUSERNAME/multi-tier-cache"  # ← UPDATE THIS
keywords = ["cache", "redis", "moka", "multi-tier", "stampede"]
categories = ["caching", "asynchronous", "database"]
readme = "README.md"
```

Replace:
- `Your Name <your.email@example.com>` with your actual name and email
- `YOURUSERNAME` with your GitHub username

---

## 🐙 Step 2: Initialize Git Repository

```bash
cd /home/thichuong/Desktop/multi-tier-cache

# Initialize Git
git init

# Add all files
git add .

# Create initial commit
git commit -m "Initial commit: multi-tier-cache v0.1.0

- Multi-tier caching (L1 Moka + L2 Redis)
- Cache stampede protection with 99.6% latency reduction
- Redis Streams support (XADD/XREAD/XREVRANGE)
- Automatic L2-to-L1 promotion
- Production-proven: 16,829+ RPS, 5.2ms latency, 95% hit rate
- 6 comprehensive examples
- Full documentation and rustdoc
"

# Check status
git status
```

---

## 🌐 Step 3: Create GitHub Repository

### Option A: Using GitHub CLI (Recommended)

```bash
# Install GitHub CLI if not already installed
# For Fedora:
sudo dnf install gh

# Login to GitHub
gh auth login

# Create repository (public)
gh repo create multi-tier-cache --public --description "High-performance multi-tier cache library for Rust with L1+L2 and stampede protection" --source=. --remote=origin --push

# Done! Repository created and code pushed
```

### Option B: Using GitHub Web Interface

1. **Go to GitHub**: https://github.com/new

2. **Fill in details**:
   - Repository name: `multi-tier-cache`
   - Description: `High-performance multi-tier cache library for Rust with L1+L2 and stampede protection`
   - Visibility: **Public**
   - **DO NOT** initialize with README, license, or .gitignore (we already have them)

3. **Click "Create repository"**

4. **Push your code**:
   ```bash
   cd /home/thichuong/Desktop/multi-tier-cache

   # Add GitHub remote
   git remote add origin https://github.com/YOURUSERNAME/multi-tier-cache.git

   # Push to GitHub
   git branch -M main
   git push -u origin main
   ```

5. **Verify**: Visit `https://github.com/YOURUSERNAME/multi-tier-cache`

---

## 🏷️ Step 4: Create Initial Release

### Using GitHub CLI

```bash
# Create a tag
git tag -a v0.1.0 -m "Release v0.1.0

Initial release of multi-tier-cache library.

Features:
- Multi-tier caching (L1 Moka + L2 Redis)
- Cache stampede protection (99.6% latency reduction)
- Redis Streams support
- Automatic L2-to-L1 promotion
- Production-proven performance (16,829+ RPS)

See README.md for full documentation."

# Push tag
git push origin v0.1.0

# Create GitHub release
gh release create v0.1.0 \
  --title "v0.1.0: Initial Release" \
  --notes "Initial release of multi-tier-cache.

## Features
- 🔥 Multi-tier caching (L1 Moka + L2 Redis)
- 🛡️ Cache stampede protection (99.6% latency reduction: 534ms → 5.2ms)
- 📊 Redis Streams support (XADD/XREAD)
- ⚡ Automatic L2-to-L1 promotion
- 📈 Comprehensive statistics and monitoring
- ✅ Production-proven: 16,829+ RPS, 5.2ms latency, 95% hit rate

## Examples
See \`examples/\` directory for 6 comprehensive examples.

## Documentation
Full documentation available at [docs.rs/multi-tier-cache](https://docs.rs/multi-tier-cache)"
```

### Using GitHub Web Interface

1. Go to: `https://github.com/YOURUSERNAME/multi-tier-cache/releases/new`
2. Tag: `v0.1.0`
3. Title: `v0.1.0: Initial Release`
4. Description: (copy from above)
5. Click "Publish release"

---

## 🚀 Step 5: Setup GitHub Actions CI/CD

Create `.github/workflows/ci.yml`:

```bash
mkdir -p /home/thichuong/Desktop/multi-tier-cache/.github/workflows
```

```yaml
# .github/workflows/ci.yml
name: CI

on:
  push:
    branches: [ main ]
  pull_request:
    branches: [ main ]

env:
  CARGO_TERM_COLOR: always
  REDIS_URL: redis://localhost:6379

jobs:
  test:
    name: Test Suite
    runs-on: ubuntu-latest

    services:
      redis:
        image: redis:7-alpine
        ports:
          - 6379:6379
        options: >-
          --health-cmd "redis-cli ping"
          --health-interval 10s
          --health-timeout 5s
          --health-retries 5

    steps:
    - uses: actions/checkout@v4

    - name: Install Rust
      uses: dtolnay/rust-toolchain@stable

    - name: Cache cargo registry
      uses: actions/cache@v4
      with:
        path: ~/.cargo/registry
        key: ${{ runner.os }}-cargo-registry-${{ hashFiles('**/Cargo.lock') }}

    - name: Cache cargo index
      uses: actions/cache@v4
      with:
        path: ~/.cargo/git
        key: ${{ runner.os }}-cargo-git-${{ hashFiles('**/Cargo.lock') }}

    - name: Cache target directory
      uses: actions/cache@v4
      with:
        path: target
        key: ${{ runner.os }}-target-${{ hashFiles('**/Cargo.lock') }}

    - name: Run tests
      run: cargo test --verbose

    - name: Run examples
      run: |
        cargo run --example basic_usage
        cargo run --example cache_strategies
        cargo run --example health_monitoring

  fmt:
    name: Rustfmt
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt
      - run: cargo fmt --all -- --check

  clippy:
    name: Clippy
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: clippy
      - run: cargo clippy -- -D warnings

  docs:
    name: Documentation
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - run: cargo doc --no-deps --all-features
```

Commit and push:

```bash
git add .github/workflows/ci.yml
git commit -m "Add CI/CD pipeline with GitHub Actions"
git push
```

---

## 📦 Step 6: Publish to crates.io (Optional)

### 6.1 Prerequisites

1. **Create crates.io account**: https://crates.io/
2. **Get API token**: https://crates.io/me
3. **Login with cargo**:
   ```bash
   cargo login YOUR_API_TOKEN_HERE
   ```

### 6.2 Pre-Publish Verification

```bash
cd /home/thichuong/Desktop/multi-tier-cache

# Check package contents
cargo package --list

# Build package
cargo package

# Test the packaged version
cargo publish --dry-run
```

### 6.3 Publish

```bash
# Publish to crates.io
cargo publish

# Wait for indexing (usually 1-2 minutes)
```

### 6.4 Post-Publish

1. **Verify publication**: https://crates.io/crates/multi-tier-cache
2. **Check documentation**: https://docs.rs/multi-tier-cache (auto-generated)
3. **Update README badges** with actual crates.io links

---

## 🔄 Step 7: Update Main Project to Use crates.io Version (Future)

After publishing to crates.io, update main project's `Cargo.toml`:

```toml
# Before (local path)
multi-tier-cache = { path = "../multi-tier-cache" }

# After (published version)
multi-tier-cache = "0.1"
```

---

## 📝 Step 8: Add Additional Files

### CONTRIBUTING.md

Create contribution guidelines:

```bash
cd /home/thichuong/Desktop/multi-tier-cache
```

```markdown
# Contributing to multi-tier-cache

Thank you for your interest in contributing!

## Getting Started

1. Fork the repository
2. Clone your fork: `git clone https://github.com/YOURUSERNAME/multi-tier-cache.git`
3. Create a branch: `git checkout -b feature/your-feature`
4. Make changes and commit
5. Push and create a Pull Request

## Development

### Prerequisites
- Rust 1.70+ (2021 edition)
- Redis server running locally

### Build
```bash
cargo build
```

### Test
```bash
cargo test
```

### Run Examples
```bash
cargo run --example basic_usage
```

## Code Style

- Run `cargo fmt` before committing
- Run `cargo clippy` and fix warnings
- Add tests for new features
- Update documentation

## Pull Request Process

1. Update README.md if adding features
2. Add examples if appropriate
3. Ensure CI passes
4. Request review from maintainers

## License

By contributing, you agree that your contributions will be licensed under MIT OR Apache-2.0.
```

### CHANGELOG.md

Track version history:

```markdown
# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0] - 2025-01-03

### Added
- Initial release of multi-tier-cache library
- Multi-tier caching with L1 (Moka) and L2 (Redis)
- Cache stampede protection with DashMap + Mutex coalescing
- Redis Streams support (XADD, XREAD, XREVRANGE)
- Automatic L2-to-L1 promotion for frequently accessed data
- Comprehensive statistics and monitoring
- 5 cache strategies (RealTime, ShortTerm, MediumTerm, LongTerm, Custom)
- 6 comprehensive examples
- Full rustdoc documentation
- MIT OR Apache-2.0 dual license

### Performance
- Production-proven: 16,829+ RPS
- Average latency: 5.2ms
- Cache hit rate: 95% (L1: 90%, L2: 75%)
- Stampede protection: 99.6% latency reduction (534ms → 5.2ms)

[Unreleased]: https://github.com/YOURUSERNAME/multi-tier-cache/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/YOURUSERNAME/multi-tier-cache/releases/tag/v0.1.0
```

### Commit and push

```bash
git add CONTRIBUTING.md CHANGELOG.md
git commit -m "Add CONTRIBUTING.md and CHANGELOG.md"
git push
```

---

## 🎯 Step 9: Post-Publishing Tasks

### Update Repository Settings (GitHub)

1. **Add topics**: Go to repository → About → Settings
   - Topics: `rust`, `cache`, `redis`, `moka`, `performance`, `async`, `tokio`

2. **Enable Discussions** (optional):
   - Settings → Features → Discussions

3. **Add Description**:
   - "High-performance multi-tier cache library for Rust with L1+L2 and stampede protection"

4. **Set Homepage**:
   - https://docs.rs/multi-tier-cache (after publishing to crates.io)

### Update README Badges

After publishing, update badges in README.md:

```markdown
[![Crates.io](https://img.shields.io/crates/v/multi-tier-cache.svg)](https://crates.io/crates/multi-tier-cache)
[![Documentation](https://docs.rs/multi-tier-cache/badge.svg)](https://docs.rs/multi-tier-cache)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](LICENSE)
[![Build Status](https://github.com/YOURUSERNAME/multi-tier-cache/workflows/CI/badge.svg)](https://github.com/YOURUSERNAME/multi-tier-cache/actions)
```

---

## 🔐 Security

### Setup Security Policy

Create `SECURITY.md`:

```markdown
# Security Policy

## Supported Versions

| Version | Supported          |
| ------- | ------------------ |
| 0.1.x   | :white_check_mark: |

## Reporting a Vulnerability

If you discover a security vulnerability, please send an email to:
**your.email@example.com**

Please include:
- Description of the vulnerability
- Steps to reproduce
- Potential impact
- Suggested fix (if available)

We will respond within 48 hours.

## Security Best Practices

When using this library:
- Always use REDIS_URL from secure environment variables
- Enable Redis AUTH in production
- Use TLS for Redis connections in production
- Regularly update dependencies
```

---

## 📊 Step 10: Setup Monitoring (Optional)

### Add shields.io Badges

```markdown
![Crates.io Downloads](https://img.shields.io/crates/d/multi-tier-cache)
![Crates.io License](https://img.shields.io/crates/l/multi-tier-cache)
![GitHub Stars](https://img.shields.io/github/stars/YOURUSERNAME/multi-tier-cache)
![GitHub Forks](https://img.shields.io/github/forks/YOURUSERNAME/multi-tier-cache)
```

### Setup Dependabot (Auto-Update Dependencies)

Create `.github/dependabot.yml`:

```yaml
version: 2
updates:
  - package-ecosystem: "cargo"
    directory: "/"
    schedule:
      interval: "weekly"
    open-pull-requests-limit: 10
```

---

## ✅ Publishing Checklist

Before announcing:

- [ ] Repository is public on GitHub
- [ ] CI/CD pipeline is green
- [ ] Documentation is deployed (docs.rs)
- [ ] Examples run successfully
- [ ] README is polished
- [ ] CHANGELOG is up-to-date
- [ ] Version tag created (v0.1.0)
- [ ] GitHub release published
- [ ] (Optional) Published to crates.io
- [ ] All badges updated with real URLs

---

## 🎉 Announce Your Library!

Share on:

1. **Reddit**:
   - r/rust: "Show HN: multi-tier-cache - High-performance multi-tier caching for Rust"
   - r/programming

2. **Twitter/X**:
   - Tag @rustlang
   - Use #rustlang #opensource

3. **Rust Community**:
   - This Week in Rust (submit to newsletter)
   - Rust Users Forum

4. **LinkedIn**:
   - Share as a project update

---

## 🔄 Future Releases

When releasing new versions:

```bash
# Update version in Cargo.toml
# Update CHANGELOG.md

# Commit changes
git add Cargo.toml CHANGELOG.md
git commit -m "Bump version to 0.2.0"

# Create tag
git tag -a v0.2.0 -m "Release v0.2.0"

# Push
git push origin main
git push origin v0.2.0

# Publish to crates.io
cargo publish

# Create GitHub release
gh release create v0.2.0 --title "v0.2.0" --notes "See CHANGELOG.md"
```

---

## 📚 Resources

- **Rust Book**: https://doc.rust-lang.org/book/
- **Cargo Book**: https://doc.rust-lang.org/cargo/
- **crates.io Publishing**: https://doc.rust-lang.org/cargo/reference/publishing.html
- **GitHub Actions**: https://docs.github.com/en/actions
- **Semantic Versioning**: https://semver.org/

---

**Good luck with your library! 🚀**

For questions, open an issue on GitHub.
