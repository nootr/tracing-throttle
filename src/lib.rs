//! # tracing-throttle
//!
//! High-performance log deduplication and rate limiting for the `tracing` ecosystem.
//!
//! This crate provides a `tracing::Layer` that suppresses repetitive log events based on
//! configurable policies. Events are deduplicated by their signature (level, message, and
//! fields), so identical log events are throttled together.
//!
//! # ⚠️ Production Readiness Warning
//!
//! **This is an MVP release (v0.1.0) with known issues:**
//! - No input validation
//! - No observability metrics
//!
//! **Not recommended for production use** without addressing these issues.
//! See the [README](https://github.com/nootr/tracing-throttle)
//! for details and the roadmap for fixes.
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
//!     .with_policy(Policy::count_based(100))
//!     .build();
//!
//! // Or customize:
//! let rate_limit = TracingRateLimitLayer::builder()
//!     .with_policy(Policy::count_based(100))
//!     .with_max_signatures(50_000)  // Custom limit
//!     .with_summary_interval(Duration::from_secs(30))
//!     .build();
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

// Domain layer - pure business logic
pub mod domain;

// Application layer - orchestration
pub mod application;

// Infrastructure layer - external adapters
pub mod infrastructure;

// Re-export commonly used types for convenience
pub use domain::{
    policy::{
        CountBasedPolicy, ExponentialBackoffPolicy, Policy, PolicyDecision, RateLimitPolicy,
        TimeWindowPolicy,
    },
    signature::EventSignature,
    summary::{SuppressionCounter, SuppressionSummary},
};

pub use application::{
    limiter::RateLimiter,
    ports::{Clock, Storage},
    registry::SuppressionRegistry,
};

pub use infrastructure::{
    clock::SystemClock,
    layer::{TracingRateLimitLayer, TracingRateLimitLayerBuilder},
    storage::ShardedStorage,
};
