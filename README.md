<h1 align="center">🎚️ tracing-throttle</h1>
<p align="center">
  High-performance log deduplication and rate limiting for the Rust `tracing` ecosystem.
</p>
<br />

[![Crates.io](https://img.shields.io/crates/v/tracing-throttle.svg)](https://crates.io/crates/tracing-throttle)
[![Documentation](https://docs.rs/tracing-throttle/badge.svg)](https://docs.rs/tracing-throttle)
[![Test](https://github.com/nootr/tracing-throttle/workflows/Test/badge.svg)](https://github.com/nootr/tracing-throttle/actions)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

<details>
    <summary>Table of contents</summary>

- [Installation](#installation)
- [Quick Start](#quick-start)
- [Configuration](#configuration)
  - [Rate Limiting Policies](#rate-limiting-policies)
  - [Eviction Strategies](#eviction-strategies)
  - [Observability & Metrics](#observability--metrics)
  - [Fail-Safe Operation](#fail-safe-operation)
  - [Memory Management](#memory-management)
- [Performance](#performance)
  - [Performance Optimization](#performance-optimization)
- [Examples](#examples)
- [Roadmap](#roadmap)
- [Development](#development)
  - [Setting Up Git Hooks](#setting-up-git-hooks)
- [Contributing](#contributing)
- [License](#license)

</details>

## Introduction

High-volume Rust applications often suffer from repetitive or bursty log events that overwhelm logging infrastructure. A single error condition can generate thousands of identical log messages per second, causing:

- **Infrastructure overload**: Log collectors and storage systems struggle under the load
- **Cost explosion**: Cloud logging services charge per event or storage volume
- **Signal loss**: Important logs get buried in noise
- **Observability gaps**: Rate limiting at the collector level discards logs silently

`tracing-throttle` solves this at the source by providing **signature-based rate limiting** as a drop-in `tracing::Layer`. Events with identical signatures (level, message, and fields) are deduplicated and throttled together, while unique events pass through unaffected.

### Why tracing-throttle?

- **🚀 High Performance**: Lock-free operations and sharded storage handle 15M+ ops/sec
- **🎯 Smart Deduplication**: Per-signature throttling means different errors are limited independently
- **🔧 Zero Config Necessary**: Sensible defaults work out of the box, extensive customization available
- **📊 Full Visibility**: Clear, human-readable summaries show exactly what events were suppressed
- **🛡️ Production Safe**: Circuit breaker fails open to preserve observability during errors
- **💾 Memory Bounded**: Advanced eviction strategies (LRU, priority-based, memory-based) prevent unbounded growth

### How It Works

The layer computes a signature for each log event based on its level, message template, target, and structured fields. Each unique signature gets its own rate limiter that applies your chosen policy (token bucket, time-window, count-based, etc.). This means:

- Identical errors are throttled together
- Different errors are limited independently
- Dynamic fields in messages don't break deduplication
- Per-signature statistics enable targeted investigation

## Installation

Add this to your `Cargo.toml`:

```toml
[dependencies]
tracing-throttle = "0.3"
tracing = "0.1.41"
tracing-subscriber = "0.3.20"
```

## Quick Start

```rust
use tracing_throttle::TracingRateLimitLayer;
use tracing_subscriber::prelude::*;

// Create a rate limit filter with safe defaults
// Defaults: 50 burst capacity, 1 token/sec (60/min), 10k max signatures with LRU eviction.
let rate_limit = TracingRateLimitLayer::new();

// Add it as a filter to your fmt layer
tracing_subscriber::registry()
    .with(tracing_subscriber::fmt::layer().with_filter(rate_limit))
    .init();

// Now your logs are rate limited!
for i in 0..1000 {
    tracing::info!("Processing item {}", i);
}
// First 50 emitted immediately (burst), then 1/sec (60/min) sustained rate
```

## Configuration

### Rate Limiting Policies

**Token Bucket (Default)**: Burst tolerance with natural recovery
```rust
Policy::token_bucket(50.0, 1.0).unwrap()
```

**Time-Window**: Allow K events per time period
```rust
Policy::time_window(10, Duration::from_secs(60)).unwrap()
```

**Count-Based**: Allow N events total (no recovery)
```rust
Policy::count_based(50).unwrap()
```

**Exponential Backoff**: Emit at exponentially increasing intervals
```rust
Policy::exponential_backoff()
```

**Custom**: Implement `RateLimitPolicy` trait for custom behavior

See the [API documentation](https://docs.rs/tracing-throttle) for details on each policy.

### Eviction Strategies

Control which signatures are kept when storage limits are reached:
- **LRU** (default) - Evict least recently used
- **Priority-based** - Keep important events (ERROR over INFO)
- **Memory-based** - Enforce byte limits
- **Combined** - Use both priority and memory constraints

See the [API documentation](https://docs.rs/tracing-throttle) and `examples/eviction.rs` for details.

### Observability & Metrics

#### Metrics

Track rate limiting behavior with built-in metrics:

```rust
let metrics = rate_limit.metrics();
println!("Allowed: {}", metrics.events_allowed());
println!("Suppressed: {}", metrics.events_suppressed());
println!("Suppression rate: {:.1}%", metrics.snapshot().suppression_rate() * 100.0);
```

#### Active Suppression Summaries

Optionally emit periodic summaries of suppressed events as log events (requires `async` feature):

```rust
let rate_limit = TracingRateLimitLayer::builder()
    .with_active_emission(true)
    .with_summary_interval(Duration::from_secs(60))
    .build()
    .unwrap();
```

See the [API documentation](https://docs.rs/tracing-throttle) for available metrics and customization options.

### Fail-Safe Operation

Uses a circuit breaker that **fails open** to preserve observability during errors. If rate limiting operations fail, all events are allowed through rather than being lost.

### Memory Management

Tracks up to **10,000 unique event signatures** by default (~2-4 MB, including event metadata for human-readable summaries). Configure via `.with_max_signatures()` for high-cardinality applications.

**Memory per signature:** ~200-400 bytes (varies with message length and field count)

See the [API documentation](https://docs.rs/tracing-throttle) for detailed memory breakdown, cardinality analysis, and configuration guidelines.

## Performance

See [BENCHMARKS.md](BENCHMARKS.md) for detailed measurements and methodology.

**Run benchmarks yourself:**
```bash
cargo bench --bench rate_limiting
```

### Performance Optimization

By default, the library captures event metadata for human-readable suppression summaries. This adds ~20-25% overhead in single-threaded scenarios. For maximum performance, disable the `human-readable` feature:

```toml
[dependencies]
tracing-throttle = { version = "0.3", default-features = false, features = ["async"] }
```

This improves performance, but summaries will show signature hashes instead of event details.

## Examples

Run the included examples:

```bash
# Basic count-based rate limiting
cargo run --example basic

# Demonstrate different policies
cargo run --example policies

# Show suppression summaries (default and custom formatters)
cargo run --example summaries --features async
```

## Roadmap

### v0.1.0 (MVP)
✅ **Completed:**
- Domain policies (count-based, time-window, exponential backoff)
- Basic registry and rate limiter
- `tracing::Layer` implementation
- LRU eviction with configurable memory limits
- Maximum signature limit with LRU eviction
- Input validation (non-zero limits, durations, reasonable max_events)
- Observability metrics (events allowed/suppressed, eviction tracking, signature count)
- Circuit breaker for fail-safe operation
- Memory usage documentation
- Comprehensive integration tests
- Mock infrastructure with test-helpers feature
- Extensive test coverage (unit, integration, and doc tests)
- Performance benchmarks (20M ops/sec)
- Hexagonal architecture (clean ports & adapters)
- CI/CD workflows (test, lint, publish)

### v0.1.1 (Production Hardening)
✅ **Completed:**
- Graceful shutdown for async emitter with `EmitterHandle`
- Comprehensive shutdown testing (12 dedicated shutdown tests)
- Explicit shutdown requirement (no Drop implementation to prevent race conditions)
- Final emission support on shutdown
- Panic safety for emit functions with proper resource cleanup
- Structured error handling with `ShutdownError` enum
- Shutdown timeout support (default 10s, customizable)
- Improved error handling and diagnostics
- Biased shutdown signal prioritization for fast shutdown
- Cancellation safety documentation

### v0.2.0 (Enhanced Observability)
✅ **Completed:**
- Active suppression summary emission (automatic WARN-level emission of suppression statistics)
- Configurable summary formatting (custom formatters for log level, fields, and message format)
- Metrics integration examples (Prometheus/OpenTelemetry integration patterns documented)

### v0.3.0 (Advanced Features)
✅ **Completed:**
- Pluggable storage backends (Redis backend with distributed rate limiting)
- Rate limit by span context (per-user, per-tenant, per-request)
- Advanced eviction policies (LRU, priority-based, memory-based, combined)

### v1.0.0 (Stability & Production Readiness)
Focus on reliability, documentation, and production hardening:
- Comprehensive integration guide and best practices
- Performance tuning guide with real-world scenarios
- Production deployment examples and patterns
- Stability testing and edge case coverage
- API stabilization and deprecation policy
- Telemetry and observability cookbook
- Performance regression testing in CI

## Development

### Setting Up Git Hooks

This project includes pre-commit hooks that run formatting, linting, tests, and example builds. To enable them:

```bash
# One-time setup - configure Git to use the .githooks directory
git config core.hooksPath .githooks
```

The pre-commit hook will automatically run:
- `cargo fmt --check` - Verify code formatting
- `cargo clippy --all-features --all-targets` - Run lints
- `cargo test --all-features` - Run all tests
- `cargo build --examples` - Build examples
- Quick smoke test of examples

## Contributing

Contributions are welcome! Please open issues or pull requests on GitHub.

## License

Licensed under the [MIT License](LICENSE).
