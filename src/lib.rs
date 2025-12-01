//! # tracing-throttle
//!
//! High-performance log deduplication and rate limiting for the `tracing` ecosystem.
//!
//! This crate provides a `tracing::Layer` that suppresses repetitive log events based on
//! configurable policies. Events are deduplicated by their signature (level, target, and message).
//! Event field **values** are NOT included in signatures by default - use
//! `.with_event_fields()` to include specific fields.
//!
//!
//! ## Quick Start
//!
//! ```rust,no_run
//! use tracing_throttle::{TracingRateLimitLayer, Policy};
//! use tracing_subscriber::prelude::*;
//! use std::time::Duration;
//!
//! // Use sensible defaults: 50 burst capacity, 1 token/sec (60/min), 10k signature limit
//! let rate_limit = TracingRateLimitLayer::new();
//!
//! // Or customize for high-volume applications:
//! let rate_limit = TracingRateLimitLayer::builder()
//!     .with_policy(Policy::token_bucket(100.0, 10.0).unwrap())  // 100 burst, 600/min
//!     .with_max_signatures(50_000)  // Custom limit
//!     .with_summary_interval(Duration::from_secs(30))
//!     .build()
//!     .unwrap();
//!
//! // Apply the rate limit as a filter to your fmt layer
//! tracing_subscriber::registry()
//!     .with(tracing_subscriber::fmt::layer().with_filter(rate_limit))
//!     .init();
//! ```
//!
//! ## Features
//!
//! - **Token bucket limiting**: Burst tolerance with smooth recovery (recommended default)
//! - **Time-window limiting**: Allow K events per time period with natural reset
//! - **Count-based limiting**: Allow N events, then suppress the rest (no recovery)
//! - **Exponential backoff**: Emit at exponentially increasing intervals (1st, 2nd, 4th, 8th...)
//! - **Custom policies**: Implement your own rate limiting logic
//! - **Per-signature throttling**: Different messages are throttled independently
//! - **LRU eviction**: Optional memory limits with automatic eviction of least recently used signatures
//! - **Observability metrics**: Built-in tracking of allowed, suppressed, and evicted events
//! - **Fail-safe circuit breaker**: Fails open during errors to preserve observability
//!
//! ## Event Signatures
//!
//! Events are deduplicated based on their **signature**. By default, signatures include:
//! - Event level (INFO, WARN, ERROR, etc.)
//! - Target (module path)
//! - Message text
//!
//! **Event field VALUES are NOT included by default.** This means:
//!
//! ```rust,no_run
//! # use tracing::info;
//! info!(user_id = 1, "Login");  // Signature: (INFO, target, "Login")
//! info!(user_id = 2, "Login");  // SAME signature - will be rate limited together!
//! ```
//!
//! To rate-limit events per field value, use `.with_event_fields()`:
//!
//! ```rust,no_run
//! # use tracing_throttle::TracingRateLimitLayer;
//! let layer = TracingRateLimitLayer::builder()
//!     .with_event_fields(vec!["user_id".to_string()])  // Include user_id in signature
//!     .build()
//!     .unwrap();
//! ```
//!
//! Now each user_id gets its own rate limit:
//!
//! ```rust,no_run
//! # use tracing::info;
//! info!(user_id = 1, "Login");  // Signature: (INFO, target, "Login", user_id=1)
//! info!(user_id = 2, "Login");  // Signature: (INFO, target, "Login", user_id=2)
//! ```
//!
//! **See `tests/event_fields.rs` for complete examples.**
//!
//! ## Observability
//!
//! Monitor rate limiting behavior with built-in metrics:
//!
//! ```rust,no_run
//! # use tracing_throttle::{TracingRateLimitLayer, Policy};
//! # let rate_limit = TracingRateLimitLayer::builder()
//! #     .with_policy(Policy::count_based(100).unwrap())
//! #     .build()
//! #     .unwrap();
//! // Get current metrics
//! let metrics = rate_limit.metrics();
//! println!("Events allowed: {}", metrics.events_allowed());
//! println!("Events suppressed: {}", metrics.events_suppressed());
//! println!("Signatures evicted: {}", metrics.signatures_evicted());
//!
//! // Get snapshot for calculations
//! let snapshot = metrics.snapshot();
//! println!("Suppression rate: {:.2}%", snapshot.suppression_rate() * 100.0);
//! ```
//!
//! ## Fail-Safe Operation
//!
//! The library uses a circuit breaker to fail open during errors, preserving
//! observability over strict rate limiting:
//!
//! ```rust,no_run
//! # use tracing_throttle::{TracingRateLimitLayer, CircuitState};
//! # let rate_limit = TracingRateLimitLayer::new();
//! // Check circuit breaker state
//! let cb = rate_limit.circuit_breaker();
//! match cb.state() {
//!     CircuitState::Closed => println!("Normal operation"),
//!     CircuitState::Open => println!("Failing open - allowing all events"),
//!     CircuitState::HalfOpen => println!("Testing recovery"),
//! }
//! ```
//!
//! ## Memory Management
//!
//! By default, tracks up to 10,000 unique event signatures with LRU eviction.
//! Each signature uses approximately 200-400 bytes (includes event metadata for summaries).
//!
//! **Typical memory usage:**
//! - 10,000 signatures (default): ~2-4 MB
//! - 50,000 signatures: ~10-20 MB
//! - 100,000 signatures: ~20-40 MB
//!
//! **Configuration:**
//! ```rust,no_run
//! # use tracing_throttle::TracingRateLimitLayer;
//! // Increase limit for high-cardinality applications
//! let rate_limit = TracingRateLimitLayer::builder()
//!     .with_max_signatures(50_000)
//!     .build()
//!     .unwrap();
//!
//! // Monitor usage
//! let sig_count = rate_limit.signature_count();
//! let evictions = rate_limit.metrics().signatures_evicted();
//! ```
//!
//! ### Memory Usage Breakdown
//!
//! Each tracked signature consumes memory for:
//!
//! ```text
//! Per-Signature Memory:
//! ├─ EventSignature (hash key)      ~32 bytes  (u64 hash)
//! ├─ EventState (value)              ~170-370 bytes
//! │  ├─ Policy state                 ~40-80 bytes (depends on policy type)
//! │  ├─ SuppressionCounter           ~40 bytes (atomic counters + timestamp)
//! │  ├─ EventMetadata (Optional)     ~50-200 bytes (level, message, target, fields)
//! │  │  ├─ Level string              ~8 bytes
//! │  │  ├─ Message string            ~20-100 bytes (depends on message length)
//! │  │  ├─ Target string             ~20-50 bytes (module path)
//! │  │  └─ Fields (BTreeMap)         ~0-50 bytes (depends on field count)
//! │  └─ Metadata overhead            ~40 bytes (DashMap internals)
//! └─ Total per signature             ~200-400 bytes (varies with policy & message length)
//! ```
//!
//! **Estimated memory usage at different signature limits:**
//!
//! | Signatures | Memory (typical) | Memory (worst case) | Use Case |
//! |------------|------------------|---------------------|----------|
//! | 1,000      | ~200 KB          | ~400 KB             | Small apps, few event types |
//! | 10,000 (default) | ~2 MB      | ~4 MB               | Most applications |
//! | 50,000     | ~10 MB           | ~20 MB              | High-cardinality apps |
//! | 100,000    | ~20 MB           | ~40 MB              | Very large systems |
//!
//! **Additional overhead:**
//! - Metrics: ~100 bytes (atomic counters)
//! - Circuit breaker: ~200 bytes (state tracking)
//! - Layer structure: ~500 bytes
//! - **Total fixed overhead: ~800 bytes**
//!
//! ### Signature Cardinality Analysis
//!
//! **What affects signature cardinality?**
//!
//! By default, signatures are computed from `(level, target, message)` only.
//! Field values are NOT included unless configured with `.with_event_fields()`.
//!
//! ```rust,no_run
//! # use tracing::info;
//! // Low cardinality (good) - same signature for all occurrences
//! info!("User login successful");  // Always same signature
//! info!(user_id = 123, "User login");  // SAME signature (user_id not included by default)
//!
//! // Medium cardinality - if you configure .with_event_fields(vec!["user_id".to_string()])
//! # let id = 123;
//! info!(user_id = %id, "User login");  // One signature per unique user_id
//!
//! // High cardinality (danger) - if you configure .with_event_fields(vec!["request_id".to_string()])
//! # let uuid = "abc";
//! info!(request_id = %uuid, "Processing");  // New signature every time!
//! ```
//!
//! **Cardinality examples:**
//!
//! | Pattern | Config | Unique Signatures | Memory Impact |
//! |---------|--------|-------------------|---------------|
//! | Static messages only | Default | ~10-100 | Minimal (~10 KB) |
//! | Messages with fields | Default (fields ignored) | ~10-100 | Minimal (~10 KB) |
//! | `.with_event_fields(["user_id"])` | Stable IDs | ~1,000-10,000 | Low (1-2 MB) |
//! | `.with_event_fields(["session_id"])` | Session IDs | ~10,000-100,000 | Medium (10-25 MB) |
//! | `.with_event_fields(["request_id"])` | UUIDs | Unbounded | **High risk** |
//!
//! **How to estimate your cardinality:**
//!
//! 1. **Count unique log templates** in your codebase
//! 2. **Multiply by field cardinality** (unique values per field)
//! 3. **Example calculation:**
//!    - 50 unique log messages
//!    - 10 severity levels used
//!    - Average 20 unique user IDs per message
//!    - **Estimated: 50 × 20 = 1,000 signatures** (✓ well below default)
//!
//! ### Configuration Guidelines
//!
//! **When to use the default (10k signatures):**
//! - ✅ Most applications with structured logging
//! - ✅ Log messages use stable identifiers (user_id, tenant_id, service_name)
//! - ✅ You're unsure about cardinality
//! - ✅ Memory is not severely constrained
//!
//! **When to increase the limit:**
//!
//! ```rust,no_run
//! # use tracing_throttle::TracingRateLimitLayer;
//! let rate_limit = TracingRateLimitLayer::builder()
//!     .with_max_signatures(50_000)  // 5-10 MB overhead
//!     .build()
//!     .expect("valid config");
//! ```
//!
//! - ✅ High log volume with many unique event types (>10k)
//! - ✅ Large distributed system with many services/endpoints
//! - ✅ You've measured cardinality and need more capacity
//! - ✅ Memory is available (10+ MB is acceptable)
//!
//! **When to use unlimited signatures:**
//!
//! ```rust,no_run
//! # use tracing_throttle::TracingRateLimitLayer;
//! let rate_limit = TracingRateLimitLayer::builder()
//!     .with_unlimited_signatures()  // ⚠️ Unbounded memory growth
//!     .build()
//!     .expect("valid config");
//! ```
//!
//! - ⚠️ **Use with extreme caution** - can cause unbounded memory growth
//! - ✅ Controlled environments (short-lived processes, tests)
//! - ✅ Known bounded cardinality with monitoring in place
//! - ✅ Memory constraints are not a concern
//! - ❌ **Never use** if logging includes UUIDs, timestamps, or other high-cardinality data
//!
//! ### Monitoring Memory Usage
//!
//! **Check signature count in production:**
//!
//! ```rust,no_run
//! # use tracing_throttle::TracingRateLimitLayer;
//! # use tracing::warn;
//! # let rate_limit = TracingRateLimitLayer::new();
//! // In a periodic health check or metrics reporter:
//! let sig_count = rate_limit.signature_count();
//! let evictions = rate_limit.metrics().signatures_evicted();
//!
//! if sig_count > 8000 {
//!     warn!("Approaching signature limit: {}/10000", sig_count);
//! }
//!
//! if evictions > 1000 {
//!     warn!("High eviction rate: {} signatures evicted", evictions);
//! }
//! ```
//!
//! **Integrate with memory profilers:**
//!
//! ```bash
//! # Use Valgrind Massif for heap profiling
//! valgrind --tool=massif --massif-out-file=massif.out ./your-app
//!
//! # Analyze with ms_print
//! ms_print massif.out
//!
//! # Look for DashMap and EventState allocations
//! ```
//!
//! **Signs you need to adjust signature limits:**
//!
//! | Symptom | Likely Cause | Action |
//! |---------|--------------|--------|
//! | High eviction rate (>1000/min) | Cardinality > limit | Increase `max_signatures` |
//! | Memory growth over time | Unbounded cardinality | Fix logging (remove UUIDs), add limit |
//! | Low signature count (<100) | Over-provisioned | Can reduce limit safely |
//! | Frequent evictions + suppression | Limit too low | Increase limit or reduce cardinality |

// Domain layer - pure business logic
pub mod domain;

// Application layer - orchestration
pub mod application;

// Infrastructure layer - external adapters
pub mod infrastructure;

// Re-export commonly used types for convenience
pub use domain::{
    policy::{
        CountBasedPolicy, ExponentialBackoffPolicy, Policy, PolicyDecision, PolicyError,
        RateLimitPolicy, TimeWindowPolicy, TokenBucketPolicy,
    },
    signature::EventSignature,
    summary::{SuppressionCounter, SuppressionSummary},
};

pub use application::{
    circuit_breaker::{CircuitBreaker, CircuitBreakerConfig, CircuitState},
    emitter::EmitterConfigError,
    limiter::RateLimiter,
    metrics::{Metrics, MetricsSnapshot},
    ports::{Clock, Storage},
    registry::SuppressionRegistry,
};

#[cfg(feature = "async")]
pub use application::emitter::{EmitterHandle, ShutdownError};

pub use infrastructure::{
    clock::SystemClock,
    eviction::{EvictionStrategy, PriorityFn},
    layer::{BuildError, TracingRateLimitLayer, TracingRateLimitLayerBuilder},
    storage::ShardedStorage,
};

#[cfg(feature = "async")]
pub use infrastructure::layer::SummaryFormatter;

#[cfg(feature = "redis-storage")]
pub use infrastructure::redis_storage::{RedisStorage, RedisStorageConfig};
