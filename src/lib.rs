//! # tracing-throttle
//!
//! High-performance log deduplication and rate limiting for the `tracing` ecosystem.
//!
//! This crate provides a `tracing::Layer` that suppresses repetitive log events based on
//! configurable policies. Events are deduplicated by their signature (level, message, and
//! fields), so identical log events are throttled together.
//!
//!
//! ## Quick Start
//!
//! ```rust,no_run
//! use tracing_throttle::{TracingRateLimitLayer, Policy};
//! use tracing_subscriber::prelude::*;
//! use std::time::Duration;
//!
//! // Uses safe defaults: 100 events, 10k signature limit
//! let rate_limit = TracingRateLimitLayer::builder()
//!     .with_policy(Policy::count_based(100).unwrap())
//!     .build()
//!     .unwrap();
//!
//! // Or customize:
//! let rate_limit = TracingRateLimitLayer::builder()
//!     .with_policy(Policy::count_based(100).unwrap())
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
//! - **Count-based limiting**: Allow N events, then suppress the rest
//! - **Time-window limiting**: Allow K events per time period
//! - **Exponential backoff**: Emit at exponentially increasing intervals (1st, 2nd, 4th, 8th...)
//! - **Custom policies**: Implement your own rate limiting logic
//! - **Per-signature throttling**: Different messages are throttled independently
//! - **LRU eviction**: Optional memory limits with automatic eviction of least recently used signatures
//! - **Observability metrics**: Built-in tracking of allowed, suppressed, and evicted events
//! - **Fail-safe circuit breaker**: Fails open during errors to preserve observability
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
        RateLimitPolicy, TimeWindowPolicy,
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

pub use infrastructure::{
    clock::SystemClock,
    layer::{BuildError, TracingRateLimitLayer, TracingRateLimitLayerBuilder},
    storage::ShardedStorage,
};
