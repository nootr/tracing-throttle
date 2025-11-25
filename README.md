# tracing-throttle

[![Crates.io](https://img.shields.io/crates/v/tracing-throttle.svg)](https://crates.io/crates/tracing-throttle)
[![Documentation](https://docs.rs/tracing-throttle/badge.svg)](https://docs.rs/tracing-throttle)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

High-performance log deduplication and rate limiting for the Rust `tracing` ecosystem.

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
- 💾 **Memory Control**: Optional LRU eviction to prevent unbounded memory growth
- 📈 **Observability Metrics**: Built-in tracking of allowed, suppressed, and evicted events
- 🛡️ **Fail-Safe Circuit Breaker**: Fails open to preserve observability during errors
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

// Create a rate limit filter with safe defaults
// Defaults: 100 events per signature, 10k max signatures with LRU eviction and a 30 second summary interval.
let rate_limit = TracingRateLimitLayer::new();

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

## Observability & Metrics

Monitor rate limiting behavior with built-in metrics:

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

## Rate Limiting Policies

### Count-Based Policy

Allow N events, then suppress all subsequent occurrences:

```rust
use tracing_throttle::Policy;

let policy = Policy::count_based(50).expect("max_count must be > 0");
// Allows first 50 events, suppresses the rest
```

### Time-Window Policy

Allow K events within a sliding time window:

```rust
use std::time::Duration;
use tracing_throttle::Policy;

let policy = Policy::time_window(10, Duration::from_secs(60))
    .expect("max_events and window must be > 0");
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
