# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2025-11-25

### Added

#### Core Features
- **Rate Limiting Policies**
  - Count-based policy: Allow N events then suppress
  - Time-window policy: Allow K events per time period
  - Exponential backoff policy: Emit at exponentially increasing intervals (1st, 2nd, 4th, 8th...)
  - Custom policy support via `RateLimitPolicy` trait

- **Event Signature System**
  - Compute signatures from (level, message, fields)
  - Per-signature throttling for independent rate limiting
  - Hash-based deduplication using ahash

- **Memory Management**
  - LRU eviction with configurable signature limits (default: 10,000)
  - Approximate LRU using sampling for performance
  - Support for unlimited signatures (with warnings)
  - Memory usage: ~150-250 bytes per signature

- **Observability & Metrics**
  - Track events allowed, suppressed, and evicted
  - `MetricsSnapshot` for point-in-time analysis
  - Suppression rate calculation
  - Signature count monitoring
  - Thread-safe atomic counters

- **Fail-Safe Circuit Breaker**
  - Three states: Closed, Open, HalfOpen
  - Fail-open strategy to preserve observability
  - Configurable failure threshold (default: 5)
  - Automatic recovery after timeout (default: 30s)
  - Panic protection using `catch_unwind`

- **tracing Integration**
  - `TracingRateLimitLayer` implementing `tracing::Layer`
  - `Filter` trait implementation for layer composition
  - Builder pattern for configuration
  - Input validation for all parameters

#### Infrastructure
- **Hexagonal Architecture**
  - Clean separation: Domain → Application → Infrastructure
  - Port & adapter pattern for Clock and Storage
  - MockClock for deterministic testing

- **Concurrency**
  - Sharded storage using DashMap (16 shards)
  - Lock-free atomic operations
  - Thread-safe across all components
  - Scales to 44M ops/sec with 8 threads

- **Testing**
  - 105 comprehensive tests (94 unit + 11 doc)
  - Integration tests for circuit breaker
  - Concurrent access stress tests
  - Edge case coverage

#### Documentation
- **README.md**
  - Quick start guide
  - Feature overview
  - Policy examples
  - Memory management summary
  - Performance benchmarks
  - Observability guide
  - Circuit breaker documentation

- **API Documentation (lib.rs)**
  - Comprehensive memory usage breakdown
  - Signature cardinality analysis
  - Configuration guidelines
  - Production monitoring examples
  - Memory profiling integration

- **Examples**
  - `basic.rs`: Simple usage example
  - `policies.rs`: Different policy demonstrations

- **Benchmarks**
  - Signature computation benchmarks
  - Single-threaded throughput tests
  - Concurrent throughput tests
  - Signature diversity scenarios
  - Registry scaling tests

#### CI/CD
- **GitHub Actions Workflows**
  - `test.yml`: Multi-OS (Ubuntu, macOS, Windows) and multi-channel (stable, beta) testing
  - `lint.yml`: Format checking, clippy, and documentation validation
  - `publish.yml`: Automated crates.io publishing on tags

### Performance

- **Throughput**
  - 20M rate limiting decisions/sec (single-threaded)
  - 44M ops/sec with 8 threads
  - Excellent scaling with concurrent access

- **Latency**
  - Signature computation: 13-37ns (simple), 200ns (20 fields)
  - Rate limit decision: ~50ns per operation

- **Memory**
  - Zero allocations in hot path
  - Lock-free operations where possible
  - Efficient sharded storage

### Dependencies

- `tracing` 0.1 - Core tracing support
- `tracing-subscriber` 0.3 - Layer implementation
- `ahash` 0.8 - Fast non-cryptographic hashing
- `dashmap` 6.0 - Concurrent hash map
- `tokio` 1.0 (optional) - Async runtime for future features

### Notes

This is the initial release of `tracing-throttle`, providing a production-ready foundation for log deduplication and rate limiting in Rust applications using the `tracing` ecosystem.

**Breaking Changes**: N/A (initial release)

**Deprecations**: None

**Known Limitations**:
- Field extraction from events is not yet implemented (signatures currently use empty fields)
- Suppression summaries planned for v0.2
- Graceful shutdown for async emitter planned for v0.2

[0.1.0]: https://github.com/nootr/tracing-throttle/releases/tag/v0.1.0
