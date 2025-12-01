//! Example demonstrating suppression summary emission with human-readable details.
//!
//! This example shows how active summary emission provides visibility into
//! suppressed events with clear, human-readable information about WHAT was suppressed,
//! not just a cryptic signature hash.

use std::time::Duration;
use tracing::{error, info, warn};
use tracing_subscriber::prelude::*;
use tracing_throttle::{Policy, TracingRateLimitLayer};

#[cfg(feature = "async")]
#[tokio::main]
async fn main() {
    run_example().await;
}

#[cfg(not(feature = "async"))]
fn main() {
    println!("This example requires the 'async' feature to be enabled.");
    println!("Run with: cargo run --example summaries --features async");
}

#[cfg(feature = "async")]
async fn run_example() {
    println!("=== Suppression Summary Example ===\n");
    println!("This example demonstrates how summaries show WHAT events are suppressed,");
    println!("not just signature hashes. You'll see clear, human-readable descriptions.\n");

    // Demo with readable summaries
    demo_default_formatter().await;

    println!("\n=== Example Complete ===");
}

#[cfg(feature = "async")]
async fn demo_default_formatter() {
    println!("Human-Readable Suppression Summaries");
    println!("-------------------------------------\n");

    // Create a rate limit layer with default summary emission
    let rate_limit_layer = TracingRateLimitLayer::builder()
        .with_policy(Policy::count_based(3).unwrap())
        .with_active_emission(true)
        .with_summary_interval(Duration::from_secs(1))
        .build()
        .unwrap();

    let layer_clone = rate_limit_layer.clone();

    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_filter(rate_limit_layer))
        .init();

    tokio::time::sleep(Duration::from_millis(100)).await;

    println!("Configuration:");
    println!("  - Policy: count-based (first 3 allowed)");
    println!("  - Summary interval: 1 seconds");
    println!("  - Formatter: default (WARN level)\n");

    println!("Scenario: High-traffic application with repetitive events\n");

    println!("📝 Emitting 20 identical user login events:");
    for _ in 1..=20 {
        info!(user_id = "user_123", "User login successful");
    }
    println!("   → First 3 allowed, rest suppressed\n");

    println!("❌ Emitting 15 identical database error events:");
    for _ in 1..=15 {
        error!(error_code = "DB_CONN_FAILED", "Database connection timeout");
    }
    println!("   → First 3 allowed, rest suppressed\n");

    println!("⚠️  Emitting 10 identical cache miss warnings:");
    for _ in 1..=10 {
        warn!(cache_key = "session:abc", "Cache miss detected");
    }
    println!("   → First 3 allowed, rest suppressed\n");

    println!("⏳ Waiting for summary emission (1.1 seconds)...");
    println!("\n🔍 Watch the summaries below - notice how they tell you EXACTLY");
    println!("   what was suppressed, not just a meaningless hash!\n");

    tokio::time::sleep(Duration::from_millis(1100)).await;

    layer_clone.shutdown().await.ok();
}
