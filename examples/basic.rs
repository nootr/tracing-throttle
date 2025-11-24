//! Basic example demonstrating count-based rate limiting.
//!
//! This example shows how to set up a simple rate limiter that allows
//! up to 3 identical log events before suppressing further occurrences.

use tracing::{info, warn};
use tracing_subscriber::prelude::*;
use tracing_throttle::{Policy, TracingRateLimitLayer};

fn main() {
    // Create a rate limit layer with a count-based policy
    // Default: max 10,000 signatures with LRU eviction
    let rate_limit_layer = TracingRateLimitLayer::builder()
        .with_policy(Policy::count_based(3))
        .build();

    // Set up the tracing subscriber with the rate limit layer as a filter
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_filter(rate_limit_layer))
        .init();

    println!("=== Basic Rate Limiting Example ===\n");
    println!("Policy: Allow first 3 occurrences, then suppress\n");

    // Emit 10 identical INFO messages
    println!("Emitting 10 identical INFO messages:");
    for i in 1..=10 {
        info!(iteration = i, "This is a repeated log message");
    }

    println!("\n");

    // Emit 10 identical WARN messages (different signature, separate limit)
    println!("Emitting 10 identical WARN messages (different signature):");
    for i in 1..=10 {
        warn!(iteration = i, "This is a repeated warning message");
    }

    println!("\n");

    // Different messages have independent limits
    println!("Emitting different messages (each has its own limit):");
    for i in 1..=5 {
        info!(iteration = i, "Message A");
        info!(iteration = i, "Message B");
        info!(iteration = i, "Message C");
    }

    println!("\n=== Example Complete ===");
    println!("Notice: Only the first 3 occurrences of each unique message are shown.");
}
