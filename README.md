# tracing-throttle

[![Crates.io](https://img.shields.io/crates/v/tracing-throttle.svg)](https://crates.io/crates/tracing-throttle)
[![Documentation](https://docs.rs/tracing-throttle/badge.svg)](https://docs.rs/tracing-throttle)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
![Status](https://img.shields.io/badge/status-MVP--not--production--ready-orange)

High-performance log deduplication and rate limiting for the Rust `tracing` ecosystem.

> **⚠️ Warning:** This is an MVP release (v0.1.0) with known issues. Not recommended for production use without addressing memory growth and correctness issues. See [Production Readiness Status](#️-production-readiness-status) below.

## Overview

`tracing-throttle` suppresses repetitive or bursty log events in high-volume systems. It helps you:

- **Reduce I/O bandwidth** from repetitive logs
- **Improve log visibility** by filtering noise
- **Lower storage costs** for log aggregation
- **Prevent log backend overload** during traffic spikes

The crate provides a `tracing::Layer` that deduplicates events based on their signature (level, message, and structured fields) and applies configurable rate limiting policies.

## Features

- 🚀 **High Performance**: Sharded maps and lock-free operations
- 🎯 **Flexible Policies**: Count-based, time-window, exponential backoff, and custom policies
- 📊 **Per-signature Throttling**: Events with identical signatures are throttled together
- ⏱️ **Suppression Summaries**: Periodic emission of suppression statistics (coming in v0.2)
- 🔧 **Easy Integration**: Drop-in `tracing::Layer` compatible with existing subscribers

## Installation

Add this to your `Cargo.toml`:

```toml
[dependencies]
tracing-throttle = "0.1"
tracing = "0.1"
tracing-subscriber = "0.3"
```

## Quick Start

```rust
use tracing_throttle::{TracingRateLimitLayer, Policy};
use tracing_subscriber::prelude::*;
use std::time::Duration;

// Create a rate limit filter
let rate_limit = TracingRateLimitLayer::builder()
    .with_policy(Policy::count_based(100))
    .with_summary_interval(Duration::from_secs(30))
    .build();

// Add it as a filter to your fmt layer
tracing_subscriber::registry()
    .with(tracing_subscriber::fmt::layer().with_filter(rate_limit))
    .init();

// Now your logs are rate limited!
for i in 0..1000 {
    tracing::info!("Processing item {}", i);
}
// Only the first 100 will be emitted
```

## Rate Limiting Policies

### Count-Based Policy

Allow N events, then suppress all subsequent occurrences:

```rust
use tracing_throttle::Policy;

let policy = Policy::count_based(50);
// Allows first 50 events, suppresses the rest
```

### Time-Window Policy

Allow K events within a sliding time window:

```rust
use std::time::Duration;
use tracing_throttle::Policy;

let policy = Policy::time_window(10, Duration::from_secs(60));
// Allows 10 events per minute
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

## How It Works

When a log event is emitted:
1. A signature is computed from the event's level, message, and fields
2. The rate limiting policy is checked for that signature
3. The event is either allowed through or suppressed
4. Suppression counts are tracked per signature

Different log messages are throttled independently, so important logs aren't suppressed just because other logs are noisy.

## Performance

Measured on Apple Silicon with comprehensive benchmarks:

**Throughput:**
- **20 million** rate limiting decisions/sec (single-threaded)
- **44 million** ops/sec with 8 threads
- Scales well with concurrent access

**Latency:**
- Signature computation: **13-37ns** (simple), **200ns** (20 fields)
- Rate limit decision: **~50ns** per operation

**Design:**
- ahash for fast non-cryptographic hashing
- DashMap for lock-free concurrent access
- Atomic operations for lock-free counters
- Zero allocations in the hot path

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

### v0.1.0 (Current - MVP Release)
✅ **Completed:**
- Domain policies (count-based, time-window, exponential backoff)
- Basic registry and rate limiter
- `tracing::Layer` implementation
- Comprehensive test suite (65 tests)
- Performance benchmarks (20M ops/sec)
- Hexagonal architecture (clean ports & adapters)

⚠️ **Known Issues (blocks production):**
- Unbounded memory growth
- Timestamp initialization bug
- No input validation
- No observability hooks
- Integer overflow risks

### v0.1.1 (Production Hardening) - NEXT
**Critical Fixes:**
- 🔧 Add maximum signature limit with LRU eviction
- 🔧 Fix OnceLock timestamp bug
- 🔧 Add input validation (non-zero limits, reasonable max_events)
- 🔧 Fix atomic memory ordering (Release/Acquire)
- 🔧 Add saturation arithmetic for overflow protection

**Major Improvements:**
- 📊 Add observability metrics (signature count, suppression rates)
- 🛡️ Add circuit breaker for fail-safe operation
- 📚 Document memory implications and limitations
- ⚙️ Add graceful shutdown for async emitter
- 🧪 Add integration tests for edge cases

### v0.2.0 (Enhanced Observability)
- Active suppression summary emission
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

## Contributing

Contributions are welcome! Please open issues or pull requests on GitHub.

## License

Licensed under the [MIT License](LICENSE).
