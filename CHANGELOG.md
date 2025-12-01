# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.1] - 2025-12-01

### Added

- **Redis Storage Backend** (behind `redis-storage` feature flag)
  - Distributed rate limiting across multiple application instances
  - Automatic TTL-based cleanup of inactive signatures
  - Connection pooling via `redis::aio::ConnectionManager`
  - Fail-safe operation (continues if Redis unavailable)
  - Custom serialization for Policy types containing Instant fields
  - Complete Redis example with Docker Compose setup
  - See `examples/redis.rs` and `examples/redis/README.md`

### Changed

- **Documentation Improvements**
  - Clarified that event field **values** are NOT included in signatures by default
  - Added new "Event Signatures" section with clear examples
  - Updated signature cardinality documentation with field behavior examples
  - Added table showing memory impact of `.with_event_fields()` configuration
  - References `tests/event_fields.rs` for working examples
  - Fixes confusion reported in [#1](https://github.com/nootr/tracing-throttle/issues/1)

## [0.2.0] - 2025-11-26

### Added

#### Enhanced Observability Features

- **Active Suppression Summary Emission** (requires `async` feature)
  - New `.with_active_emission(bool)` builder method to enable automatic emission of suppression summaries
  - Summaries emitted as structured WARN-level tracing events at configurable intervals
  - Background task managed via `EmitterHandle` in spawned tokio task
  - Disabled by default (opt-in to prevent surprise behavior)
  - Graceful shutdown via `.shutdown().await` method

- **Configurable Summary Formatting**
  - New `SummaryFormatter` type: `Arc<dyn Fn(&SuppressionSummary) + Send + Sync + 'static>`
  - New `.with_summary_formatter()` builder method for full control over emission format
  - Customize log level, message format, and structured fields
  - Default formatter preserves existing behavior (WARN level with signature/count fields)
  - Completely optional and backward compatible

- **Token Bucket Rate Limiting Policy**
  - New default policy: `Policy::token_bucket(capacity, refill_rate)`
  - Provides burst tolerance with natural recovery over time
  - Replaces count-based policy as the recommended default
  - Default configuration: 50 burst capacity, 1 token/sec (60/min sustained)
  - Handles edge cases: time going backwards, fractional token accumulation
  - 16 comprehensive tests including critical regression tests

- **Metrics Integration Examples**
  - Prometheus integration pattern documented in `metrics` module
  - OpenTelemetry integration pattern documented in `metrics` module
  - Examples show periodic export using `snapshot()` method
  - No additional dependencies required (examples use `ignore` attribute)

### Changed

- **Breaking**: Default rate limiting policy changed from `count_based(100)` to `token_bucket(50.0, 1.0)`
  - Provides better behavior for intermittent issues (natural recovery)
  - Users relying on count-based behavior must explicitly configure it
  - Migration: Use `.with_policy(Policy::count_based(100).unwrap())` to restore old behavior

- **Builder Structure**: Removed `Debug` derive from `TracingRateLimitLayerBuilder`
  - Required to support function pointer field (`summary_formatter`)
  - Does not affect normal usage (builders are rarely debugged)

### Improved

- **Documentation**: Moved implementation details from README to API documentation
  - README is now ~170 lines shorter and more scannable
  - Rate Limiting Policies section condensed (all policies in same format)
  - Observability & Metrics section simplified
  - Fail-Safe Operation reduced to one paragraph
  - Memory Management reduced to two sentences with link to docs
  - Added links to docs.rs for detailed information

- **Code Quality**
  - All 160 tests passing (123 unit + 9 integration + 4 shutdown + 24 doc + 2 ignored)
  - Zero clippy warnings
  - Comprehensive test coverage for new features

## [0.1.1] - 2025-11-25

### Added

#### Graceful Shutdown System
- **EmitterHandle**: New handle type for controlling background emitter tasks
  - `shutdown().await`: Graceful shutdown with default 10-second timeout
  - `shutdown_with_timeout()`: Custom timeout support for flexible deadline control
  - `is_running()`: Check if emitter task is still active
  - Explicit shutdown requirement (no Drop implementation to prevent race conditions)

- **Structured Error Handling**
  - `ShutdownError` enum with clear error types:
    - `TaskPanicked`: Emitter task panicked during shutdown
    - `TaskCancelled`: Task was cancelled before completion
    - `Timeout`: Shutdown exceeded specified timeout
    - `SignalFailed`: Failed to send shutdown signal
  - All errors properly surfaced to callers (no silent failures in production)

- **Shutdown Features**
  - Final emission support on shutdown (configurable via `emit_final` parameter)
  - Biased shutdown signal prioritization for fast, deterministic shutdown
  - Panic safety with proper resource cleanup
  - Comprehensive cancellation safety documentation

### Changed

- **Breaking**: `EmitterHandle::shutdown()` now returns `Result<(), ShutdownError>` instead of `()`
  - Users must handle the Result (e.g., `.await?` or `.await.expect("shutdown failed")`)
  - Enables proper error handling in production applications

- **Breaking**: `SummaryEmitter::start()` signature changed to include `emit_final` parameter
  - Old: `start(emit_fn) -> EmitterHandle`
  - New: `start(emit_fn, emit_final: bool) -> EmitterHandle`

- **Breaking**: Removed `Drop` implementation from `EmitterHandle` to prevent race conditions
  - Users must explicitly call `shutdown().await` to stop emitter tasks
  - Prevents resource leaks and undefined behavior when tasks outlive handles

### Improved

- **Error Handling**: Production builds now properly surface all errors instead of only logging in debug mode
- **Shutdown Reliability**:
  - Biased `tokio::select!` ensures shutdown signal is checked first
  - Prevents non-deterministic delays (up to 30 seconds) during shutdown
  - Fast shutdown even under heavy load
- **Documentation**:
  - Added cancellation safety guarantees for spawned tasks
  - Documented panic handling and resource cleanup semantics
  - Clear examples showing proper shutdown patterns
  - Type parameter documentation explaining `Send + 'static` requirements
- **Memory Safety**: Added comments explaining Rust's drop semantics ensure no memory leaks even during panics

### Fixed

- **Critical (P0)**: Shutdown race condition where Drop could signal shutdown without waiting for task completion
- **Critical (P0)**: Non-deterministic shutdown delays by prioritizing shutdown signal in select! loop
- **Critical (P0)**: Missing cancellation safety documentation
- **Important (P1)**: Potential resource leaks if emitter task outlived the handle
- **Important (P1)**: Errors swallowed in production builds
- **Important (P1)**: No timeout support for hanging emit functions

### Testing

- Added 12 dedicated shutdown tests (now 133 total tests: 102 unit + 9 rate limiting + 4 shutdown + 18 doc)
- Comprehensive edge case coverage:
  - Panic recovery in emit functions (task continues after panic)
  - Custom timeout behavior
  - Concurrent shutdown safety (multiple emitters)
  - Shutdown during active emission
  - Final emission on shutdown
  - Explicit shutdown requirement
- All tests pass with zero clippy warnings

### Dependencies

- Updated to latest stable versions:
  - `tracing` 0.1 → 0.1.41
  - `tracing-subscriber` 0.3 → 0.3.20
  - `ahash` 0.8 → 0.8.12
  - `dashmap` 6.0 → 6.1
  - `tokio` 1 → 1.48

### Notes

This release focuses on production hardening with robust shutdown semantics. All P0 (critical) and P1 (important) issues from code review have been addressed. The crate is now battle-tested and ready for production use.

**Migration Guide** (from v0.1.0):

```rust
// Before (v0.1.0) - if you were using the async emitter
let handle = emitter.start(|summaries| {
    // emit logic
});
drop(handle); // Shutdown via Drop (unsafe)

// After (v0.1.1) - explicit shutdown with error handling
let handle = emitter.start(|summaries| {
    // emit logic
}, false); // false = don't emit final summaries

// Proper shutdown
handle.shutdown().await?; // Returns Result

// Or with custom timeout
handle.shutdown_with_timeout(Duration::from_secs(5)).await?;
```

**Note**: Most users are not affected by breaking changes, as the async emitter functionality was added in v0.1.0 but not fully exposed or documented. The `TracingRateLimitLayer` API remains unchanged.

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
