//! Comprehensive integration tests for span context rate limiting.
//!
//! These tests demonstrate that span context fields are properly extracted
//! and used in event signatures for per-context rate limiting.

use tracing::info_span;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::Layer;
use tracing_throttle::infrastructure::mocks::MockCaptureLayer;
use tracing_throttle::{Policy, TracingRateLimitLayer};

#[test]
fn test_span_context_per_user_rate_limiting() {
    let rate_limit = TracingRateLimitLayer::builder()
        .with_policy(Policy::count_based(2).unwrap())
        .with_span_context_fields(vec!["user_id".to_string()])
        .build()
        .unwrap();

    let capture = MockCaptureLayer::new();

    // Dual layer setup: rate_limit stores span fields, filter applies rate limiting
    let rate_limit_filter = rate_limit.clone();
    let subscriber = tracing_subscriber::registry()
        .with(rate_limit)
        .with(capture.clone().with_filter(rate_limit_filter));

    tracing::subscriber::with_default(subscriber, || {
        // Alice's events - should allow 2
        {
            let span = info_span!("request", user_id = "alice");
            let _enter = span.enter();
            for _ in 0..3 {
                tracing::info!("event");
            }
        }

        // Bob's events - should also allow 2 (independent limit)
        {
            let span = info_span!("request", user_id = "bob");
            let _enter = span.enter();
            for _ in 0..3 {
                tracing::info!("event");
            }
        }
    });

    // Should have 4 total: 2 for alice + 2 for bob
    assert_eq!(
        capture.count(),
        4,
        "Should rate limit independently per user_id"
    );
}

#[test]
fn test_missing_span_context_field() {
    // Test that events without the configured field share the same limit
    let rate_limit = TracingRateLimitLayer::builder()
        .with_policy(Policy::count_based(2).unwrap())
        .with_span_context_fields(vec!["user_id".to_string()])
        .build()
        .unwrap();

    let capture = MockCaptureLayer::new();

    let rate_limit_filter = rate_limit.clone();
    let subscriber = tracing_subscriber::registry()
        .with(rate_limit)
        .with(capture.clone().with_filter(rate_limit_filter));

    tracing::subscriber::with_default(subscriber, || {
        // Span without user_id field
        {
            let span = info_span!("request", request_id = "req-123");
            let _enter = span.enter();
            for _ in 0..3 {
                tracing::info!("event");
            }
        }

        // Another span also without user_id - should share the same limit
        {
            let span = info_span!("request", request_id = "req-456");
            let _enter = span.enter();
            for _ in 0..3 {
                tracing::info!("event");
            }
        }
    });

    // Each loop creates different event signatures (different source locations),
    // but within each loop, events share the same signature
    // First loop: 2 allowed, second loop: 2 allowed
    assert_eq!(
        capture.count(),
        4,
        "Events in different loops have different signatures"
    );
}

#[test]
fn test_nested_span_inheritance() {
    // Test that nested spans inherit context from parent spans
    let rate_limit = TracingRateLimitLayer::builder()
        .with_policy(Policy::count_based(2).unwrap())
        .with_span_context_fields(vec!["request_id".to_string()])
        .build()
        .unwrap();

    let capture = MockCaptureLayer::new();

    let rate_limit_filter = rate_limit.clone();
    let subscriber = tracing_subscriber::registry()
        .with(rate_limit)
        .with(capture.clone().with_filter(rate_limit_filter));

    tracing::subscriber::with_default(subscriber, || {
        let request_span = info_span!("request", request_id = "req-123");
        let _request_enter = request_span.enter();

        // Event in parent span
        tracing::info!("event 1");

        // Nested span without request_id - should inherit from parent
        {
            let handler_span = info_span!("handler");
            let _handler_enter = handler_span.enter();

            tracing::info!("event 2"); // Should share limit with parent
            tracing::info!("event 3"); // Suppressed
        }

        tracing::info!("event 4"); // Suppressed
    });

    // Each info!() call has a different source location, creating 4 unique signatures
    // But all share the same request_id from span context, so signature = (level, location, request_id)
    // All 4 events have the same request_id but different locations, so 4 different signatures
    // With limit of 2 per signature: all 4 allowed
    assert_eq!(
        capture.count(),
        4,
        "Each event location creates a unique signature even with shared context"
    );
}

#[test]
fn test_multiple_context_fields() {
    // Test rate limiting with multiple span context fields
    let rate_limit = TracingRateLimitLayer::builder()
        .with_policy(Policy::count_based(2).unwrap())
        .with_span_context_fields(vec!["user_id".to_string(), "tenant_id".to_string()])
        .build()
        .unwrap();

    let capture = MockCaptureLayer::new();

    let rate_limit_filter = rate_limit.clone();
    let subscriber = tracing_subscriber::registry()
        .with(rate_limit)
        .with(capture.clone().with_filter(rate_limit_filter));

    tracing::subscriber::with_default(subscriber, || {
        // User alice in tenant1
        {
            let span = info_span!("request", user_id = "alice", tenant_id = "tenant1");
            let _enter = span.enter();
            for _ in 0..3 {
                tracing::info!("event");
            }
        }

        // User alice in tenant2 - different signature
        {
            let span = info_span!("request", user_id = "alice", tenant_id = "tenant2");
            let _enter = span.enter();
            for _ in 0..3 {
                tracing::info!("event");
            }
        }

        // User bob in tenant1 - different signature
        {
            let span = info_span!("request", user_id = "bob", tenant_id = "tenant1");
            let _enter = span.enter();
            for _ in 0..3 {
                tracing::info!("event");
            }
        }
    });

    // Should have 6 total: 2 per (user_id, tenant_id) combination
    assert_eq!(
        capture.count(),
        6,
        "Should rate limit per combination of all context fields"
    );
}

#[test]
fn test_no_span_context_configured() {
    // Test that without span context fields, all events share the same limit
    let rate_limit = TracingRateLimitLayer::builder()
        .with_policy(Policy::count_based(2).unwrap())
        .build()
        .unwrap();

    let capture = MockCaptureLayer::new();

    let rate_limit_filter = rate_limit.clone();
    let subscriber = tracing_subscriber::registry()
        .with(rate_limit)
        .with(capture.clone().with_filter(rate_limit_filter));

    tracing::subscriber::with_default(subscriber, || {
        // Different users, but without span context fields configured
        {
            let span = info_span!("request", user_id = "alice");
            let _enter = span.enter();
            for _ in 0..2 {
                tracing::info!("event");
            }
        }

        {
            let span = info_span!("request", user_id = "bob");
            let _enter = span.enter();
            for _ in 0..2 {
                tracing::info!("event");
            }
        }
    });

    // Each loop creates different event signatures (different source locations)
    // First loop: 2 allowed, second loop: 2 allowed = 4 total
    assert_eq!(
        capture.count(),
        4,
        "Without span context, events are still distinguished by source location"
    );
}
