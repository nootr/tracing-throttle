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

#[test]
fn test_explicit_parent_span_is_used_for_context() {
    // Events with an explicit `parent:` must be bucketed by that span's fields,
    // not by whichever span happens to be entered on the current thread.
    let rate_limit = TracingRateLimitLayer::builder()
        .with_policy(Policy::count_based(1).unwrap())
        .with_span_context_fields(vec!["user_id".to_string()])
        .build()
        .unwrap();

    let capture = MockCaptureLayer::new();
    let subscriber = tracing_subscriber::registry().with(capture.clone().with_filter(rate_limit));

    tracing::subscriber::with_default(subscriber, || {
        let alice = info_span!("request", user_id = "alice");
        let bob = info_span!("request", user_id = "bob");

        // Alice stays entered the whole time; bob's span is never entered.
        // A single callsite emits for both so only the parent differs.
        let _enter = alice.enter();
        for span in [&alice, &bob] {
            for _ in 0..3 {
                tracing::info!(parent: span.id(), "event");
            }
        }
    });

    assert_eq!(
        capture.count(),
        2,
        "alice and bob should each get their own bucket"
    );
}

#[test]
fn test_root_event_ignores_entered_span() {
    // `parent: None` makes an event a root; it must not absorb the entered
    // span's fields.
    let rate_limit = TracingRateLimitLayer::builder()
        .with_policy(Policy::count_based(1).unwrap())
        .with_span_context_fields(vec!["user_id".to_string()])
        .build()
        .unwrap();

    let capture = MockCaptureLayer::new();
    let subscriber = tracing_subscriber::registry().with(capture.clone().with_filter(rate_limit));

    tracing::subscriber::with_default(subscriber, || {
        for user in ["alice", "bob"] {
            let span = info_span!("request", user_id = user);
            let _enter = span.enter();
            tracing::info!(parent: None, "event");
        }
    });

    assert_eq!(
        capture.count(),
        1,
        "root events share one bucket regardless of the entered span"
    );
}

#[test]
fn test_span_record_after_creation_reaches_context() {
    // The idiomatic pattern: declare the field as Empty and record it once
    // the value is known (e.g. after authentication).
    let rate_limit = TracingRateLimitLayer::builder()
        .with_policy(Policy::count_based(2).unwrap())
        .with_span_context_fields(vec!["user_id".to_string()])
        .build()
        .unwrap();

    let capture = MockCaptureLayer::new();
    let subscriber = tracing_subscriber::registry().with(capture.clone().with_filter(rate_limit));

    tracing::subscriber::with_default(subscriber, || {
        for user in ["alice", "bob"] {
            let span = info_span!("request", user_id = tracing::field::Empty);
            let _enter = span.enter();
            span.record("user_id", user);
            for _ in 0..3 {
                tracing::info!("event");
            }
        }
    });

    assert_eq!(
        capture.count(),
        4,
        "values recorded via Span::record must be part of the span context"
    );
}

#[test]
fn test_span_record_overrides_creation_value() {
    let rate_limit = TracingRateLimitLayer::builder()
        .with_policy(Policy::count_based(1).unwrap())
        .with_span_context_fields(vec!["user_id".to_string()])
        .build()
        .unwrap();

    let capture = MockCaptureLayer::new();
    let subscriber = tracing_subscriber::registry().with(capture.clone().with_filter(rate_limit));

    tracing::subscriber::with_default(subscriber, || {
        let span = info_span!("request", user_id = "anonymous");
        let _enter = span.enter();
        for user in [None, Some("alice"), Some("bob")] {
            if let Some(user) = user {
                span.record("user_id", user);
            }
            tracing::info!("event");
        }
    });

    assert_eq!(
        capture.count(),
        3,
        "each recorded value should open a new bucket"
    );
}
