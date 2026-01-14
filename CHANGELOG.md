# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.4.1] - 2026-01-14

### Performance

**Zero-Copy Field Extraction**

Implemented zero-copy field name extraction using `Cow<'static, str>`, reducing allocations by 50% for typical logging patterns.

**Changes:**
- Field names from tracing macros are now borrowed (`&'static str`) instead of cloned
- Reduces allocations from 6 to 3 per event with 3 fields
- No measurable performance regression in signature computation (~40ns)
- Real-world benefit: reduced allocator pressure under high-volume logging

**Impact:** Transparent optimization - normal user API unchanged.

### BREAKING CHANGES (Internal API)

**EventSignature::new() signature changed**

The `EventSignature::new()` method now accepts `BTreeMap<Cow<'static, str>, Cow<'static, str>>` instead of `BTreeMap<String, String>`.

**Who's affected:** Only advanced users who directly construct `EventSignature` objects. The builder API and normal logging usage are completely unchanged.

**Migration:**
```rust
// Before (v0.4.0)
let fields = BTreeMap::from([
    ("user".to_string(), "alice".to_string()),
]);
let sig = EventSignature::new("INFO", "test", &fields, None);

// After (v0.4.1) - use Cow
use std::borrow::Cow;
let fields = BTreeMap::from([
    (Cow::Borrowed("user"), Cow::Borrowed("alice")),
]);
let sig = EventSignature::new("INFO", "test", &fields, None);

// Or: Use the unchanged simple() method
let sig = EventSignature::simple("INFO", "test");
```

**Technical details:**
- `FieldVisitor` now uses `Cow::Borrowed(field.name())` for zero-copy
- `EventMetadata` fields updated to use Cow type
- All extraction methods in `TracingRateLimitLayer` updated
- 236 tests passing, including 9 new Cow-specific tests

## [0.4.0] - 2025-12-03

### Thanks

- [@LifeMoroz](https://github.com/LifeMoroz) for reporting the field inclusion confusion in [#1](https://github.com/nootr/tracing-throttle/issues/1).

### BREAKING CHANGES

**Field Inclusion Logic Inverted**

Event field values are now **included in signatures by default** (previously excluded by default).

This breaking change addresses a fundamental UX issue: the old behavior was unintuitive and dangerous. When users wrote `info!(user_id = 123, "User login")`, they expected `user_id=123` and `user_id=456` to be treated as different events - because they ARE semantically different events. The old behavior silently ignored field values, making logs appear broken.

**Before (v0.3.x):**
```rust
// Fields EXCLUDED by default - counter-intuitive!
let layer = TracingRateLimitLayer::builder()
    .with_policy(Policy::count_based(5).unwrap())
    .build()
    .unwrap();

info!(user_id = 123, "User login");  // Same signature
info!(user_id = 456, "User login");  // Same signature (WRONG - throttled together!)

// Had to opt-in to include fields:
let layer = TracingRateLimitLayer::builder()
    .with_event_fields(vec!["user_id".to_string()])  // Explicit inclusion
    .build()
    .unwrap();
```

**After (v0.4.0):**
```rust
// Fields INCLUDED by default - intuitive and correct!
let layer = TracingRateLimitLayer::builder()
    .with_policy(Policy::count_based(5).unwrap())
    .build()
    .unwrap();

info!(user_id = 123, "User login");  // Signature includes user_id=123
info!(user_id = 456, "User login");  // Different signature (CORRECT!)

// Opt-out for high-cardinality fields:
let layer = TracingRateLimitLayer::builder()
    .with_excluded_fields(vec!["request_id".to_string(), "trace_id".to_string()])
    .build()
    .unwrap();
```

**Migration Guide:**

1. **If you were using default configuration** (no `.with_event_fields()`):
   - Signatures now include ALL field values
   - Identify high-cardinality fields (request_id, trace_id, timestamps, UUIDs)
   - Exclude them explicitly:
     ```rust
     .with_excluded_fields(vec![
         "request_id".to_string(),
         "trace_id".to_string(),
         "span_id".to_string(),
     ])
     ```

2. **If you were using `.with_event_fields()`**:
   - Remove `.with_event_fields()` calls (method no longer exists)
   - Invert the logic: exclude all OTHER fields instead:
     ```rust
     // Before:
     .with_event_fields(vec!["user_id".to_string()])

     // After - exclude everything EXCEPT user_id:
     .with_excluded_fields(vec![
         "request_id".to_string(),
         "timestamp".to_string(),
         // ... list all OTHER fields
     ])
     ```
   - **Note:** If you were including many fields, it's now simpler to exclude the few high-cardinality ones

3. **Memory Management Considerations:**
   - Monitor signature count with `.snapshot().signature_count()`
   - Set appropriate `.with_max_signatures()` limit based on cardinality
   - See updated documentation for cardinality analysis

### Removed

- **`.with_event_fields()`** - Replaced by `.with_excluded_fields()` (inverted logic)

### Added

- **`.with_excluded_fields()`** - Opt-out from including specific fields in signatures
  - Use for high-cardinality fields (request_id, trace_id, UUIDs, timestamps)
  - Prevents signature explosion
  - Example: `.with_excluded_fields(vec!["request_id".to_string()])`

### Changed

- **Default signature computation** - Now includes `(level, target, message, ALL field values)`
  - Previously: `(level, target, message)` only
  - Field values define semantic meaning, so they belong in signatures
  - More intuitive: `user_id=123` and `user_id=456` are different events

### Documentation

- **lib.rs module documentation** - Updated to reflect new field inclusion behavior
- **Quick Start example** - Added `.with_excluded_fields()` demonstration
- **Memory Management section** - Rewrote cardinality analysis for new behavior
- **Cardinality table** - Updated examples showing exclusion patterns
- **All code examples** - Consistently use `.with_excluded_fields()` API
- **examples/basic.rs** - Updated to use `.with_excluded_fields()`
- **examples/policies.rs** - Updated to use `.with_excluded_fields()`

### Fixed

- **Unintuitive field handling** - Field values now correctly included by default
- **Silent field value ignoring** - Users no longer surprised by fields being excluded
- **Documentation consistency** - All docs now consistently describe v0.4.0 behavior

### Rationale

This breaking change was necessary because:

1. **Principle of Least Surprise**: When users write `info!(user_id = 123, "Login")`, they expect the `user_id` value to matter. Ignoring it by default violated this principle.

2. **Semantic Correctness**: Field values define the semantic meaning of events. `user_id=123` and `user_id=456` are fundamentally different events and should be throttled independently.

3. **Safer Defaults**: The old behavior could silently cause incorrect throttling. The new behavior is correct by default, with explicit opt-out for performance tuning.

4. **User Feedback**: Real-world testing revealed confusion about why fields were being ignored, leading to perceived bugs in log output.

5. **Rust Idioms**: Explicit exclusion follows Rust's philosophy: be explicit about what you DON'T want, rather than what you DO want.

## [0.3.1] - 2025-12-02

### Added

- **Comprehensive Test Coverage** (51 new tests, +2,482 lines)
  - Circuit breaker: race conditions, boundary conditions, clone behavior
  - Emitter: panic recovery, callback ordering, error handling
  - Limiter: fail-open behavior, policy application, concurrent access
  - Storage: memory tracking accuracy, eviction sampling edge cases
  - Redis storage: serialization, TTL, concurrent access, prefix isolation
  - Total test count: 276 tests (225 unit + 39 integration + 12 Redis)

### Fixed

- Circuit breaker test flakiness due to timing sensitivity
- Clone behavior tests now correctly validate independent atomic state
- Removed unnecessary `.clone()` calls on Copy types (clippy warnings)
- Fixed Policy API usage in tests (time_window and exponential_backoff signatures)

### Changed

- Removed JSON serialization tests (Redis uses bincode, not JSON)
- Simplified policy serialization tests to match actual implementation

## [0.3.0] - 2025-12-01

### Added

- **Advanced Eviction Strategies** (4 strategies total)
  - LRU eviction (default) - evicts least recently used signatures
  - Priority-based eviction - custom function determines importance
  - Memory-based eviction - enforces byte limits with lock-free tracking
  - Combined priority+memory - uses both constraints simultaneously
  - New `.with_eviction_strategy()` builder method
  - Sampling-based eviction (5-20 samples) for O(1) amortized performance
  - Conservative memory estimation (~200 bytes per signature)

- **Human-Readable Suppression Summaries**
  - Event details (level, target, message) included in suppression summaries
  - Makes it easier to identify which events were suppressed
  - Helpful for quick diagnostics without needing to look up signature hashes

### Changed

- **Documentation Restructure**
  - Added comprehensive eviction examples to lib.rs API docs
  - Organized features into categories (policies, eviction, other)
  - Simplified README eviction section, refer to docs for details
  - Updated "Why tracing-throttle?" to mention eviction strategies
  - Refocused v1.0.0 roadmap on stability and production readiness

### Improved

- **Performance**: Updated benchmarks showing 15M+ ops/sec with advanced eviction
- **Memory tracking**: Atomic memory accounting for lock-free operations
- **Testing**: 9 new integration tests for eviction strategies (42 total tests)

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
