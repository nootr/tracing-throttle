# 🎛️ tracing-throttle

[![Crates.io](https://img.shields.io/crates/v/tracing-throttle.svg)](https://crates.io/crates/tracing-throttle)
[![Documentation](https://docs.rs/tracing-throttle/badge.svg)](https://docs.rs/tracing-throttle)
[![Test](https://github.com/nootr/tracing-throttle/workflows/Test/badge.svg)](https://github.com/nootr/tracing-throttle/actions)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

High-performance log deduplication and rate limiting for the Rust `tracing` ecosystem.

## Introduction

High-volume Rust applications often suffer from repetitive or bursty log events that overwhelm logging infrastructure. A single error condition can generate thousands of identical log messages per second, causing:

- **Infrastructure overload**: Log collectors and storage systems struggle under the load
- **Cost explosion**: Cloud logging services charge per event or storage volume
- **Signal loss**: Important logs get buried in noise
- **Observability gaps**: Rate limiting at the collector level discards logs silently

`tracing-throttle` solves this at the source by providing **signature-based rate limiting** as a drop-in `tracing::Layer`. Events with identical signatures (level, message, and fields) are deduplicated and throttled together, while unique events pass through unaffected.

### Why tracing-throttle?

- **🚀 High Performance**: Lock-free operations and sharded storage handle 20M+ ops/sec
- **🎯 Smart Deduplication**: Per-signature throttling means different errors are limited independently
- **🔧 Zero Config**: Sensible defaults work out of the box, extensive customization available
- **📊 Full Visibility**: Built-in metrics track what's being suppressed and why
- **🛡️ Production Safe**: Circuit breaker fails open to preserve observability during errors
- **💾 Memory Bounded**: LRU eviction prevents unbounded growth in high-cardinality scenarios

### How It Works

The layer computes a signature for each log event based on its level, message template, target, and structured fields. Each unique signature gets its own rate limiter that applies your chosen policy (token bucket, time-window, count-based, etc.). This means:

- Identical errors are throttled together
- Different errors are limited independently
- Dynamic fields in messages don't break deduplication
- Per-signature statistics enable targeted investigation

## Table of Contents

- [Installation](#installation)
- [Quick Start](#quick-start)
- [Rate Limiting Policies](#rate-limiting-policies)
  - [Token Bucket Policy (Default)](#token-bucket-policy-default)
  - [Time-Window Policy](#time-window-policy)
  - [Count-Based Policy](#count-based-policy)
  - [Exponential Backoff Policy](#exponential-backoff-policy)
  - [Custom Policies](#custom-policies)
- [Observability & Metrics](#observability--metrics)
- [Fail-Safe Operation](#fail-safe-operation)
- [Memory Management](#memory-management)
- [Performance](#performance)
- [Examples](#examples)
- [Roadmap](#roadmap)
- [Development](#development)
- [Contributing](#contributing)
- [License](#license)

## Installation

Add this to your `Cargo.toml`:

```toml
[dependencies]
tracing-throttle = "0.1.1"
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

## Rate Limiting Policies

### Token Bucket Policy (Default)

Best choice for intermittent issues - allows bursts but recovers naturally:

```rust
use tracing_throttle::Policy;

// Default: moderate rate limiting (50 burst, 60/min sustained)
let policy = Policy::token_bucket(50.0, 1.0)
    .expect("capacity and refill_rate must be > 0");

// High-volume applications: increase limits
let high_volume = Policy::token_bucket(100.0, 10.0).unwrap();
// Allows bursts of up to 100 events
// Refills at 10 tokens/second (sustained rate: 600/min)
```

**Benefits:**
- **Burst tolerance**: Handle spikes up to capacity
- **Natural recovery**: Tokens refill over time - issues that settle down return to normal
- **Smooth rate limiting**: No hard cutoffs
- **Forgiveness**: Intermittent problems don't permanently suppress logs
- **Industry standard**: Same algorithm used by AWS, GCP, Kubernetes

**Typical configurations:**
- **Default (50, 1.0)**: 3,600 events/hour max - good for most applications
- **High-volume (100, 10.0)**: 36,000 events/hour max - for high-throughput systems
- **Strict (20, 0.5)**: 1,800 events/hour max - for aggressive rate limiting

### Time-Window Policy

Allow K events within a sliding time window:

```rust
use std::time::Duration;
use tracing_throttle::Policy;

let policy = Policy::time_window(10, Duration::from_secs(60))
    .expect("max_events and window must be > 0");
// Allows 10 events per minute
// Window naturally resets as time passes
```

### Count-Based Policy

Allow N events, then suppress all subsequent occurrences:

```rust
use tracing_throttle::Policy;

let policy = Policy::count_based(50).expect("max_count must be > 0");
// Allows first 50 events, suppresses the rest
// ⚠️ No recovery mechanism - once limit hit, suppressed forever
```

### Exponential Backoff Policy

Emit events at exponentially increasing intervals (1st, 2nd, 4th, 8th, 16th, ...):

```rust
use tracing_throttle::Policy;

let policy = Policy::exponential_backoff();
// Useful for extremely noisy logs
```

### Custom Policies

Implement the `RateLimitPolicy` trait for custom behavior:

```rust
use tracing_throttle::RateLimitPolicy;
use std::time::Instant;

struct MyCustomPolicy;

impl RateLimitPolicy for MyCustomPolicy {
    fn register_event(&mut self, timestamp: Instant) -> PolicyDecision {
        // Your custom logic here
    }

    fn reset(&mut self) {
        // Reset policy state
    }
}
```

## Observability & Metrics

### Active Suppression Summaries

Enable automatic emission of suppression statistics as `WARN`-level log events:

```rust
use tracing_throttle::TracingRateLimitLayer;
use std::time::Duration;

let rate_limit = TracingRateLimitLayer::builder()
    .with_active_emission(true)
    .with_summary_interval(Duration::from_secs(60))
    .build()
    .expect("valid config");

// Suppression summaries will now be automatically emitted every 60 seconds
// Example output: "Suppressed 1,234 events (signature: EventSignature(123456789))"
```

**Requires the `async` feature.** Summaries are emitted as structured WARN-level tracing events with fields:
- `signature` - The event signature hash
- `count` - Number of suppressions since last emission or reset

### Metrics API

Monitor rate limiting behavior programmatically with built-in metrics:

```rust
use tracing_throttle::{TracingRateLimitLayer, Policy};

let rate_limit = TracingRateLimitLayer::builder()
    .with_policy(Policy::count_based(100).expect("valid policy"))
    .build()
    .expect("valid config");

// ... after some log events have been processed ...

// Get current metrics
let metrics = rate_limit.metrics();
println!("Events allowed: {}", metrics.events_allowed());
println!("Events suppressed: {}", metrics.events_suppressed());
println!("Signatures evicted: {}", metrics.signatures_evicted());

// Or get a snapshot for calculations
let snapshot = metrics.snapshot();
println!("Total events: {}", snapshot.total_events());
println!("Suppression rate: {:.2}%", snapshot.suppression_rate() * 100.0);

// Check how many unique signatures are being tracked
println!("Tracked signatures: {}", rate_limit.signature_count());
```

**Available Metrics:**
- `events_allowed()` - Total events allowed through
- `events_suppressed()` - Total events suppressed
- `signatures_evicted()` - Signatures removed due to LRU eviction
- `signature_count()` - Current number of tracked signatures
- `suppression_rate()` - Ratio of suppressed to total events (0.0 - 1.0)

**Use Cases:**
- Monitor suppression rates in production dashboards
- Alert when suppression rate exceeds threshold
- Track signature cardinality growth
- Observe LRU eviction frequency
- Validate rate limiting effectiveness

## Fail-Safe Operation

`tracing-throttle` uses a circuit breaker pattern to prevent cascading failures. If rate limiting operations fail (e.g., panics or internal errors), the library **fails open** to preserve observability:

- **Closed**: Normal operation, rate limiting active
- **Open**: After threshold failures (default: 5), fails open and allows all events
- **HalfOpen**: After recovery timeout (default: 30s), tests if system has recovered
- **Fail-Open Strategy**: Preserves observability over strict rate limiting

This ensures your logs remain visible during system instability, preventing silent data loss.

## Memory Management

By default, the layer tracks up to **10,000 unique event signatures** with LRU eviction. Each signature uses approximately **150-250 bytes**.

**Typical memory usage:**
- 10,000 signatures (default): **~1.5-2.5 MB**
- 50,000 signatures: **~7.5-12.5 MB**
- 100,000 signatures: **~15-25 MB**

```rust
// Increase limit for high-cardinality applications
let rate_limit = TracingRateLimitLayer::builder()
    .with_max_signatures(50_000)
    .build()
    .expect("valid config");

// Monitor usage in production
let sig_count = rate_limit.signature_count();
let evictions = rate_limit.metrics().signatures_evicted();
```

📖 **See [detailed memory documentation](https://docs.rs/tracing-throttle/latest/tracing_throttle/#memory-management) for:**
- Memory breakdown and overhead calculations
- Signature cardinality analysis and estimation
- Configuration guidelines for different use cases
- Production monitoring and profiling techniques

## Performance

See [BENCHMARKS.md](BENCHMARKS.md) for detailed measurements and methodology.

**Run benchmarks yourself:**
```bash
cargo bench --bench rate_limiting
```

## Examples

Run the included examples:

```bash
# Basic count-based rate limiting
cargo run --example basic

# Demonstrate different policies
cargo run --example policies
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

🚧 **In Progress:**
- Metrics adapters (Prometheus/OTel)
- Configurable summary formatting
- Memory usage telemetry

### v0.3.0 (Advanced Features)
- Pluggable storage backends (Redis, etc.)
- Streaming-friendly summaries
- Rate limit by span context
- Advanced eviction policies

### v1.0.0 (Stable Release)
- Stable API guarantee
- Production-ready documentation
- Optional WASM support
- Performance regression testing

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
