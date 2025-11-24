//! Example demonstrating different rate limiting policies.
//!
//! This example shows how to use various built-in policies:
//! - Count-based: Allow N events then suppress
//! - Time-window: Allow K events per time period
//! - Exponential backoff: Allow 1st, 2nd, 4th, 8th, etc.

use std::time::Duration;
use tracing::info;
use tracing_subscriber::prelude::*;
use tracing_throttle::{Policy, TracingRateLimitLayer};

fn demonstrate_count_based() {
    println!("\n=== Count-Based Policy ===");
    println!("Allow first 5 events, then suppress all\n");

    let layer = TracingRateLimitLayer::builder()
        .with_policy(Policy::count_based(5))
        .build();

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
        .with_policy(Policy::time_window(3, Duration::from_millis(100)))
        .build();

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
        .build();

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

    demonstrate_count_based();
    demonstrate_time_window();
    demonstrate_exponential_backoff();

    println!("\n=== Examples Complete ===");
}
