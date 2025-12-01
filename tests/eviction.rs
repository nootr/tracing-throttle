//! Integration tests for eviction strategies.

use std::sync::Arc;
use tracing::{error, info};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::Layer;
use tracing_throttle::infrastructure::mocks::MockCaptureLayer;
use tracing_throttle::{EvictionStrategy, Policy, TracingRateLimitLayer};

#[test]
fn test_lru_eviction_with_max_signatures() {
    // Create a layer with a very small signature limit
    let capture = MockCaptureLayer::new();
    let layer = TracingRateLimitLayer::builder()
        .with_policy(Policy::token_bucket(1000.0, 100.0).unwrap())
        .with_max_signatures(5) // Only allow 5 signatures
        .build()
        .unwrap();

    let subscriber =
        tracing_subscriber::registry().with(capture.clone().with_filter(layer.clone()));

    tracing::subscriber::with_default(subscriber, || {
        // Generate 10 unique signatures - each with different message template
        info!("Message 0");
        info!("Message 1");
        info!("Message 2");
        info!("Message 3");
        info!("Message 4");
        info!("Message 5");
        info!("Message 6");
        info!("Message 7");
        info!("Message 8");
        info!("Message 9");
    });

    // Should have evicted approximately 5 signatures (10 - 5 = 5)
    let evictions = layer.metrics().signatures_evicted();
    assert!(
        (4..=6).contains(&evictions),
        "Expected ~5 evictions, got {}",
        evictions
    );

    // Verify signature count is at the limit
    assert_eq!(layer.signature_count(), 5);
}

#[test]
fn test_priority_eviction_keeps_high_priority() {
    // Create a layer with priority-based eviction that favors ERROR over INFO
    let capture = MockCaptureLayer::new();
    let layer = TracingRateLimitLayer::builder()
        .with_policy(Policy::token_bucket(1000.0, 100.0).unwrap())
        .with_max_signatures(3)
        .with_eviction_strategy(EvictionStrategy::Priority {
            max_entries: 3,
            priority_fn: Arc::new(|_sig, state| {
                // Higher priority for ERROR events
                match state.metadata.as_ref().map(|m| m.level.as_str()) {
                    Some("ERROR") => 100,
                    Some("WARN") => 50,
                    Some("INFO") => 10,
                    _ => 5,
                }
            }),
        })
        .build()
        .unwrap();

    let subscriber =
        tracing_subscriber::registry().with(capture.clone().with_filter(layer.clone()));

    tracing::subscriber::with_default(subscriber, || {
        // First, emit ERROR events to fill the cache with high-priority items
        error!("Error message 1");
        error!("Error message 2");

        // Add one INFO message
        info!("Info message 1");

        // All three should fit
        assert_eq!(layer.signature_count(), 3);

        // Now add more INFO messages - they should evict existing INFO, not ERROR
        info!("Info message 2");
        info!("Info message 3");
    });

    // Still at limit
    assert_eq!(layer.signature_count(), 3);

    // The INFO messages should have been evicted, ERROR should remain
    let evictions = layer.metrics().signatures_evicted();
    assert!(
        evictions >= 2,
        "Expected at least 2 evictions (INFO messages), got {}",
        evictions
    );
}

#[test]
fn test_memory_based_eviction() {
    // Create a layer with memory-based eviction (small limit to trigger eviction)
    let capture = MockCaptureLayer::new();
    let layer = TracingRateLimitLayer::builder()
        .with_policy(Policy::token_bucket(1000.0, 100.0).unwrap())
        .with_eviction_strategy(EvictionStrategy::Memory {
            max_bytes: 800, // Small enough that ~4-5 entries will exceed it
        })
        .build()
        .unwrap();

    let subscriber =
        tracing_subscriber::registry().with(capture.clone().with_filter(layer.clone()));

    tracing::subscriber::with_default(subscriber, || {
        // Generate unique messages to exceed memory limit
        info!("Msg 0");
        info!("Msg 1");
        info!("Msg 2");
        info!("Msg 3");
        info!("Msg 4");
        info!("Msg 5");
        info!("Msg 6");
        info!("Msg 7");
        info!("Msg 8");
        info!("Msg 9");
    });

    // Should have triggered evictions due to memory pressure
    let evictions = layer.metrics().signatures_evicted();
    assert!(
        evictions > 0,
        "Expected evictions due to memory limit, got 0"
    );

    // Signature count should be limited by memory, not unbounded
    let sig_count = layer.signature_count();
    assert!(
        sig_count < 10,
        "Expected memory limit to constrain signatures, got {}",
        sig_count
    );
}

#[test]
fn test_priority_with_memory_combined() {
    // Create a layer with both priority and memory limits
    let capture = MockCaptureLayer::new();
    let layer = TracingRateLimitLayer::builder()
        .with_policy(Policy::token_bucket(1000.0, 100.0).unwrap())
        .with_eviction_strategy(EvictionStrategy::PriorityWithMemory {
            max_entries: 10,
            priority_fn: Arc::new(|_sig, state| {
                match state.metadata.as_ref().map(|m| m.level.as_str()) {
                    Some("ERROR") => 100,
                    Some("WARN") => 50,
                    Some("INFO") => 10,
                    _ => 5,
                }
            }),
            max_bytes: 1000, // Very small memory limit
        })
        .build()
        .unwrap();

    let subscriber =
        tracing_subscriber::registry().with(capture.clone().with_filter(layer.clone()));

    tracing::subscriber::with_default(subscriber, || {
        // First emit ERROR events
        error!("Err A");
        error!("Err B");
        error!("Err C");

        // Then emit INFO events - should trigger eviction due to memory
        info!("Info A");
        info!("Info B");
        info!("Info C");
        info!("Info D");
        info!("Info E");
    });

    // Should have evicted some signatures
    let evictions = layer.metrics().signatures_evicted();
    assert!(evictions > 0, "Expected evictions, got 0");

    // Lower priority INFO messages should be evicted first
    // We can't directly verify which were kept, but evictions should have occurred
}

#[test]
fn test_eviction_strategy_tracks_memory() {
    let lru = EvictionStrategy::Lru { max_entries: 100 };
    assert!(!lru.tracks_memory());

    let priority = EvictionStrategy::Priority {
        max_entries: 100,
        priority_fn: Arc::new(|_, _| 1),
    };
    assert!(!priority.tracks_memory());

    let memory = EvictionStrategy::Memory { max_bytes: 1000 };
    assert!(memory.tracks_memory());

    let combined = EvictionStrategy::PriorityWithMemory {
        max_entries: 100,
        priority_fn: Arc::new(|_, _| 1),
        max_bytes: 1000,
    };
    assert!(combined.tracks_memory());
}

#[test]
fn test_eviction_strategy_memory_limit() {
    let lru = EvictionStrategy::Lru { max_entries: 100 };
    assert_eq!(lru.memory_limit(), None);

    let memory = EvictionStrategy::Memory { max_bytes: 5000 };
    assert_eq!(memory.memory_limit(), Some(5000));

    let combined = EvictionStrategy::PriorityWithMemory {
        max_entries: 100,
        priority_fn: Arc::new(|_, _| 1),
        max_bytes: 10000,
    };
    assert_eq!(combined.memory_limit(), Some(10000));
}

#[test]
fn test_eviction_strategy_uses_priority() {
    let lru = EvictionStrategy::Lru { max_entries: 100 };
    assert!(!lru.uses_priority());

    let priority = EvictionStrategy::Priority {
        max_entries: 100,
        priority_fn: Arc::new(|_, _| 1),
    };
    assert!(priority.uses_priority());

    let memory = EvictionStrategy::Memory { max_bytes: 1000 };
    assert!(!memory.uses_priority());

    let combined = EvictionStrategy::PriorityWithMemory {
        max_entries: 100,
        priority_fn: Arc::new(|_, _| 1),
        max_bytes: 1000,
    };
    assert!(combined.uses_priority());
}

#[test]
fn test_no_eviction_with_unlimited_signatures() {
    // Create a layer with unlimited signatures (no eviction)
    let capture = MockCaptureLayer::new();
    let layer = TracingRateLimitLayer::builder()
        .with_policy(Policy::token_bucket(1000.0, 100.0).unwrap())
        .with_unlimited_signatures()
        .build()
        .unwrap();

    let subscriber =
        tracing_subscriber::registry().with(capture.clone().with_filter(layer.clone()));

    tracing::subscriber::with_default(subscriber, || {
        // Generate unique signatures
        info!("Unique 0");
        info!("Unique 1");
        info!("Unique 2");
        info!("Unique 3");
        info!("Unique 4");
        info!("Unique 5");
        info!("Unique 6");
        info!("Unique 7");
        info!("Unique 8");
        info!("Unique 9");
    });

    // Should not have evicted any signatures
    assert_eq!(layer.metrics().signatures_evicted(), 0);

    // All signatures should be tracked
    assert_eq!(layer.signature_count(), 10);
}

#[test]
fn test_lru_evicts_oldest_accessed() {
    // Create a layer with small limit
    let capture = MockCaptureLayer::new();
    let layer = TracingRateLimitLayer::builder()
        .with_policy(Policy::token_bucket(1000.0, 100.0).unwrap())
        .with_max_signatures(3)
        .build()
        .unwrap();

    let subscriber =
        tracing_subscriber::registry().with(capture.clone().with_filter(layer.clone()));

    tracing::subscriber::with_default(subscriber, || {
        // Create 3 signatures
        info!("Message A");
        info!("Message B");
        info!("Message C");

        assert_eq!(layer.signature_count(), 3);

        // Access A and B to refresh them
        info!("Message A");
        info!("Message B");

        // Now add new signatures - should evict C and potentially others (LRU)
        info!("Message D");
        info!("Message E");
        info!("Message F");

        // Should still be at limit
        assert_eq!(layer.signature_count(), 3);

        // Should have evicted at least 3 signatures to make room
        let evictions = layer.metrics().signatures_evicted();
        assert!(
            evictions >= 3,
            "Expected at least 3 evictions, got {}",
            evictions
        );
    });
}
