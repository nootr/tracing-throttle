//! Example demonstrating different rate limiting policies.
//!
//! This example shows how to use various built-in policies:
//! - Token bucket: Burst tolerance with natural recovery (recommended)
//! - Time-window: Allow K events per time period
//! - Count-based: Allow N events then suppress
//! - Exponential backoff: Allow 1st, 2nd, 4th, 8th, etc.
//!
//! Note: All examples exclude the `iteration` field from signatures.
//! This is because `iteration` is just tracking loop progress, not part of
//! the event's semantic meaning. Excluding it allows us to demonstrate
//! throttling behavior on truly repeated events.

use std::time::Duration;
use tracing::info;
use tracing_subscriber::prelude::*;
use tracing_throttle::{Policy, TracingRateLimitLayer};

// Helper function to log events from the same source location
fn log_event(iteration: i32) {
    info!(iteration = iteration, "Token bucket test message");
}

fn demonstrate_token_bucket() {
    println!("\n=== Token Bucket Policy (Recommended) ===");
    println!("Allow bursts of 5, refill at 2 tokens/sec\n");

    let layer = TracingRateLimitLayer::builder()
        .with_policy(Policy::token_bucket(5.0, 2.0).unwrap())
        .with_excluded_fields(vec!["iteration".to_string()])
        .build()
        .unwrap();

    let subscriber =
        tracing_subscriber::registry().with(tracing_subscriber::fmt::layer().with_filter(layer));

    tracing::subscriber::with_default(subscriber, || {
        // First burst - 8 events immediately (only 5 allowed)
        println!("First burst (8 events immediately):");
        for i in 1..=8 {
            log_event(i);
        }
        println!("  (Expected: 5 allowed, 3 suppressed)");

        // Wait 1 second - should refill 2 tokens
        std::thread::sleep(Duration::from_secs(1));

        // Second burst - should allow 2 more
        println!("\nSecond burst after 1 second (5 more events):");
        for i in 9..=13 {
            log_event(i);
        }
        println!("  (Expected: 2 allowed, 3 suppressed)");

        // Wait 3 seconds - should refill to full capacity (need 5 tokens at 2/sec = 2.5s minimum)
        std::thread::sleep(Duration::from_secs(3));

        // Third burst - should allow 5 more (full capacity restored)
        println!("\nThird burst after 3 more seconds (5 more events):");
        for i in 14..=18 {
            log_event(i);
        }
        println!("  (Expected: 5 allowed, full capacity restored)");
    });
}

fn demonstrate_count_based() {
    println!("\n=== Count-Based Policy ===");
    println!("Allow first 5 events, then suppress all\n");

    let layer = TracingRateLimitLayer::builder()
        .with_policy(Policy::count_based(5).unwrap())
        .with_excluded_fields(vec!["iteration".to_string()])
        .build()
        .unwrap();

    let subscriber =
        tracing_subscriber::registry().with(tracing_subscriber::fmt::layer().with_filter(layer));

    tracing::subscriber::with_default(subscriber, || {
        for i in 1..=10 {
            info!(iteration = i, "Count-based test message");
        }
    });
}

fn demonstrate_time_window() {
    println!("\n=== Time-Window Policy ===");
    println!("Allow 3 events per 100ms window\n");

    let layer = TracingRateLimitLayer::builder()
        .with_policy(Policy::time_window(3, Duration::from_millis(100)).unwrap())
        .with_excluded_fields(vec!["iteration".to_string()])
        .build()
        .unwrap();

    let subscriber =
        tracing_subscriber::registry().with(tracing_subscriber::fmt::layer().with_filter(layer));

    tracing::subscriber::with_default(subscriber, || {
        // First burst - 5 events immediately
        println!("First burst (5 events):");
        for i in 1..=5 {
            info!(iteration = i, "Time-window test message");
        }

        // Wait for window to expire
        std::thread::sleep(Duration::from_millis(150));

        // Second burst - should allow 3 more
        println!("\nSecond burst after 150ms (5 more events):");
        for i in 6..=10 {
            info!(iteration = i, "Time-window test message");
        }
    });
}

fn demonstrate_exponential_backoff() {
    println!("\n=== Exponential Backoff Policy ===");
    println!("Allow events at exponentially increasing intervals\n");
    println!("Allows: 1st, 2nd, 4th, 8th, 16th, 32nd, ...\n");

    let layer = TracingRateLimitLayer::builder()
        .with_policy(Policy::exponential_backoff())
        .with_excluded_fields(vec!["iteration".to_string()])
        .build()
        .unwrap();

    let subscriber =
        tracing_subscriber::registry().with(tracing_subscriber::fmt::layer().with_filter(layer));

    tracing::subscriber::with_default(subscriber, || {
        for i in 1..=20 {
            info!(iteration = i, "Exponential backoff test message");
        }
    });
}

fn main() {
    println!("=== Built-in Policy Examples ===");

    demonstrate_token_bucket();
    demonstrate_time_window();
    demonstrate_count_based();
    demonstrate_exponential_backoff();

    println!("\n=== Examples Complete ===");
}
