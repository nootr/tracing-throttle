use tracing::{info, warn, Level};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::Layer;
use tracing_throttle::infrastructure::mocks::MockCaptureLayer;
use tracing_throttle::{Policy, TracingRateLimitLayer};

#[test]
fn test_rate_limit_layer_integration() {
    let capture = MockCaptureLayer::new();
    let rate_limit = TracingRateLimitLayer::builder()
        .with_policy(Policy::count_based(3).unwrap())
        .build()
        .unwrap();

    let subscriber = tracing_subscriber::registry().with(capture.clone().with_filter(rate_limit));

    tracing::subscriber::with_default(subscriber, || {
        // Emit 10 identical events
        for _ in 0..10 {
            info!("test event");
        }
    });

    // Only first 3 should pass through
    assert_eq!(capture.count(), 3);
}

#[test]
fn test_different_messages_independent_limits() {
    let capture = MockCaptureLayer::new();
    let rate_limit = TracingRateLimitLayer::builder()
        .with_policy(Policy::count_based(2).unwrap())
        .build()
        .unwrap();

    let subscriber = tracing_subscriber::registry().with(capture.clone().with_filter(rate_limit));

    tracing::subscriber::with_default(subscriber, || {
        // Each message type gets its own limit
        for _ in 0..5 {
            info!("message A");
        }
        for _ in 0..5 {
            warn!("message B");
        }
    });

    // 2 from A + 2 from B = 4 total
    assert_eq!(capture.count(), 4);
}

#[test]
fn test_different_levels_independent_limits() {
    let capture = MockCaptureLayer::new();
    let rate_limit = TracingRateLimitLayer::builder()
        .with_policy(Policy::count_based(2).unwrap())
        .build()
        .unwrap();

    let subscriber = tracing_subscriber::registry().with(capture.clone().with_filter(rate_limit));

    tracing::subscriber::with_default(subscriber, || {
        for _ in 0..5 {
            info!("same message");
        }
        for _ in 0..5 {
            warn!("same message");
        }
    });

    let captured = capture.get_captured();

    // 2 info + 2 warn = 4 total
    assert_eq!(captured.len(), 4);

    // Verify levels
    let info_count = captured.iter().filter(|e| e.level == Level::INFO).count();
    let warn_count = captured.iter().filter(|e| e.level == Level::WARN).count();

    assert_eq!(info_count, 2);
    assert_eq!(warn_count, 2);
}

#[test]
fn test_metrics_match_suppression() {
    let capture = MockCaptureLayer::new();
    let rate_limit = TracingRateLimitLayer::builder()
        .with_policy(Policy::count_based(3).unwrap())
        .build()
        .unwrap();

    let subscriber =
        tracing_subscriber::registry().with(capture.clone().with_filter(rate_limit.clone()));

    tracing::subscriber::with_default(subscriber, || {
        for _ in 0..10 {
            info!("test");
        }
    });

    // Verify metrics match actual suppression
    let metrics = rate_limit.metrics().snapshot();
    assert_eq!(metrics.events_allowed, 3);
    assert_eq!(metrics.events_suppressed, 7);
    assert_eq!(capture.count(), 3); // Only allowed events captured
}

#[test]
fn test_signature_count_tracking() {
    let capture = MockCaptureLayer::new();
    let rate_limit = TracingRateLimitLayer::builder()
        .with_policy(Policy::count_based(1).unwrap())
        .build()
        .unwrap();

    let subscriber = tracing_subscriber::registry().with(capture.with_filter(rate_limit.clone()));

    tracing::subscriber::with_default(subscriber, || {
        info!("message 1");
        info!("message 2");
        info!("message 3");
        info!("message 1"); // Same callsite, so same signature
    });

    // Each info!() creates a unique callsite, so 4 unique signatures
    // (tracing treats each macro invocation as a unique event type)
    assert_eq!(rate_limit.signature_count(), 4);
}

#[test]
fn test_eviction_with_small_limit() {
    let capture = MockCaptureLayer::new();
    let rate_limit = TracingRateLimitLayer::builder()
        .with_policy(Policy::count_based(1).unwrap())
        .with_max_signatures(3)
        .build()
        .unwrap();

    let subscriber = tracing_subscriber::registry().with(capture.with_filter(rate_limit.clone()));

    tracing::subscriber::with_default(subscriber, || {
        // Fill to capacity
        info!("msg 1");
        info!("msg 2");
        info!("msg 3");

        // This should trigger eviction
        info!("msg 4");
    });

    // Signature count should be at limit
    assert_eq!(rate_limit.signature_count(), 3);

    // Should have evicted 1 signature
    assert_eq!(rate_limit.metrics().signatures_evicted(), 1);
}

#[test]
fn test_circuit_breaker_remains_closed() {
    use tracing_throttle::CircuitState;

    let rate_limit = TracingRateLimitLayer::builder()
        .with_policy(Policy::count_based(2).unwrap())
        .build()
        .unwrap();

    let subscriber = tracing_subscriber::registry().with(rate_limit.clone());

    tracing::subscriber::with_default(subscriber, || {
        for _ in 0..100 {
            info!("test");
        }
    });

    // Circuit breaker should remain closed during normal operation
    assert_eq!(rate_limit.circuit_breaker().state(), CircuitState::Closed);
    assert_eq!(rate_limit.circuit_breaker().consecutive_failures(), 0);
}

#[test]
fn test_high_concurrency_stress() {
    use std::thread;

    let capture = MockCaptureLayer::new();
    let rate_limit = TracingRateLimitLayer::builder()
        .with_policy(Policy::count_based(100).unwrap())
        .build()
        .unwrap();

    let subscriber =
        tracing_subscriber::registry().with(capture.clone().with_filter(rate_limit.clone()));

    tracing::subscriber::set_global_default(subscriber).ok();

    let mut handles = vec![];

    // Spawn 10 threads, each emitting the same message 50 times
    for _ in 0..10 {
        let handle = thread::spawn(|| {
            for _ in 0..50 {
                info!("concurrent test");
            }
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.join().unwrap();
    }

    // Total: 10 threads × 50 events = 500 events
    // Should allow max 100
    let metrics = rate_limit.metrics().snapshot();
    assert_eq!(metrics.events_allowed, 100);
    assert_eq!(metrics.events_suppressed, 400);
    assert_eq!(capture.count(), 100);
}

#[test]
fn test_zero_limit_suppresses_all_after_first() {
    let capture = MockCaptureLayer::new();
    let rate_limit = TracingRateLimitLayer::builder()
        .with_policy(Policy::count_based(1).unwrap())
        .build()
        .unwrap();

    let subscriber = tracing_subscriber::registry().with(capture.clone().with_filter(rate_limit));

    tracing::subscriber::with_default(subscriber, || {
        for _ in 0..100 {
            info!("test");
        }
    });

    // Only the very first event allowed
    assert_eq!(capture.count(), 1);
}
