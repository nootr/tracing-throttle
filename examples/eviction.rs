//! Example demonstrating different eviction strategies.
//!
//! Run with: `cargo run --example eviction`

use std::sync::Arc;
use tracing::{error, info, warn};
use tracing_subscriber::prelude::*;
use tracing_throttle::{EvictionStrategy, Policy, TracingRateLimitLayer};

fn main() {
    println!("=== Eviction Strategies Example ===\n");

    // Example 1: Default LRU eviction
    println!("1. Default LRU (Least Recently Used) Eviction:");
    println!("   - Tracks up to 5 signatures");
    println!("   - Evicts oldest when limit reached\n");

    let layer1 = TracingRateLimitLayer::builder()
        .with_policy(Policy::token_bucket(1000.0, 100.0).unwrap())
        .with_max_signatures(5)
        .build()
        .unwrap();

    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_filter(layer1.clone()))
        .init();

    // Generate 7 unique signatures - should evict 2
    info!("Msg 0");
    info!("Msg 1");
    info!("Msg 2");
    info!("Msg 3");
    info!("Msg 4");
    info!("Msg 5");
    info!("Msg 6");

    let sig_count = layer1.signature_count();
    let evictions = layer1.metrics().signatures_evicted();

    println!("   Signatures tracked: {} (max 5)", sig_count);
    println!("   Signatures evicted: {}\n", evictions);

    // Verify eviction occurred
    assert_eq!(sig_count, 5, "Should track exactly 5 signatures at limit");
    assert!(evictions >= 1, "Should have evicted at least 2 signatures");

    // Reset subscriber for next example
    drop(layer1);

    // Example 2: Priority-based eviction
    println!("2. Priority-Based Eviction:");
    println!("   - ERROR events have priority 100");
    println!("   - WARN events have priority 50");
    println!("   - INFO events have priority 10");
    println!("   - Lower priority evicted first\n");

    let layer2 = TracingRateLimitLayer::builder()
        .with_policy(Policy::token_bucket(1000.0, 100.0).unwrap())
        .with_max_signatures(3)
        .with_eviction_strategy(EvictionStrategy::Priority(Arc::new(|_sig, state| {
            // Prioritize by log level
            match state.metadata.as_ref().map(|m| m.level.as_str()) {
                Some("ERROR") => 100,
                Some("WARN") => 50,
                Some("INFO") => 10,
                _ => 5,
            }
        })))
        .build()
        .unwrap();

    // Create a new subscriber with the new layer
    let subscriber2 = tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_filter(layer2.clone()));

    tracing::subscriber::with_default(subscriber2, || {
        // Fill with ERROR events
        error!("Critical error 1");
        error!("Critical error 2");

        // Add INFO event
        info!("Info message");

        let sig_count_1 = layer2.signature_count();
        println!("   After 2 ERRORs and 1 INFO: {} signatures", sig_count_1);
        assert_eq!(sig_count_1, 3, "Should have 3 signatures");

        // Add more INFO - should evict INFO, not ERROR
        info!("Info message 2");
        info!("Info message 3");

        let sig_count_2 = layer2.signature_count();
        let evictions = layer2.metrics().signatures_evicted();

        println!("   After 3 more INFOs: {} signatures", sig_count_2);
        println!("   Evictions: {}", evictions);
        println!("   (INFO messages evicted, ERROR messages kept)\n");

        assert_eq!(sig_count_2, 3, "Should still have 3 signatures");
        assert!(
            evictions >= 2,
            "Should have evicted at least 2 INFO messages"
        );
    });

    // Example 3: Memory-based eviction
    println!("3. Memory-Based Eviction:");
    println!("   - Max memory: 800 bytes");
    println!("   - Evicts when memory limit exceeded\n");

    let layer3 = TracingRateLimitLayer::builder()
        .with_policy(Policy::token_bucket(1000.0, 100.0).unwrap())
        .with_eviction_strategy(EvictionStrategy::Memory { max_bytes: 800 })
        .build()
        .unwrap();

    let subscriber3 = tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_filter(layer3.clone()));

    tracing::subscriber::with_default(subscriber3, || {
        // Generate signatures until memory limit hit
        info!("Mem0");
        info!("Mem1");
        info!("Mem2");
        info!("Mem3");
        info!("Mem4");
        info!("Mem5");
        info!("Mem6");
        info!("Mem7");
        info!("Mem8");
        info!("Mem9");

        let sig_count = layer3.signature_count();
        let evictions = layer3.metrics().signatures_evicted();

        println!("   Signatures tracked: {}", sig_count);
        println!("   Memory-triggered evictions: {}", evictions);
        println!("   (Eviction triggered by memory, not count)\n");

        assert!(sig_count < 10, "Memory limit should constrain signatures");
        assert!(evictions > 0, "Should have triggered memory-based eviction");
    });

    // Example 4: Combined priority and memory limits
    println!("4. Combined Priority + Memory Eviction:");
    println!("   - Uses priority function (ERROR > WARN > INFO)");
    println!("   - Also enforces 1000 byte memory limit");
    println!("   - Evicts lowest priority when either limit exceeded\n");

    let layer4 = TracingRateLimitLayer::builder()
        .with_policy(Policy::token_bucket(1000.0, 100.0).unwrap())
        .with_eviction_strategy(EvictionStrategy::PriorityWithMemory {
            priority_fn: Arc::new(|_sig, state| {
                match state.metadata.as_ref().map(|m| m.level.as_str()) {
                    Some("ERROR") => 100,
                    Some("WARN") => 50,
                    Some("INFO") => 10,
                    _ => 5,
                }
            }),
            max_bytes: 1000,
        })
        .build()
        .unwrap();

    let subscriber4 = tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_filter(layer4.clone()));

    tracing::subscriber::with_default(subscriber4, || {
        // Mix of priority levels
        error!("Err0");
        error!("Err1");
        error!("Err2");
        warn!("Warn0");
        warn!("Warn1");
        warn!("Warn2");
        info!("Info0");
        info!("Info1");
        info!("Info2");
        info!("Info3");
        info!("Info4");

        let sig_count = layer4.signature_count();
        let evictions = layer4.metrics().signatures_evicted();

        println!("   Signatures tracked: {}", sig_count);
        println!("   Evictions: {}", evictions);
        println!("   (Low-priority INFO evicted first to stay under memory limit)\n");

        assert!(
            sig_count < 11,
            "Combined limits should constrain signatures"
        );
        assert!(evictions > 0, "Should have triggered eviction");
    });

    println!("=== Summary ===");
    println!("Eviction strategies allow fine-grained control over which");
    println!("event signatures are kept when storage limits are reached:");
    println!("  - LRU: Keep recently-used events (default, good for most cases)");
    println!("  - Priority: Keep important events (ERROR > INFO)");
    println!("  - Memory: Control memory usage explicitly");
    println!("  - Combined: Best of both priority and memory constraints");
}
