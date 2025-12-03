//! Integration tests for event field behavior.
//!
//! Tests that ALL event fields are included in signatures by default,
//! and that excluded_fields works correctly to remove high-cardinality fields.

use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::Layer;
use tracing_throttle::infrastructure::mocks::MockCaptureLayer;
use tracing_throttle::{Policy, TracingRateLimitLayer};

#[test]
fn test_all_fields_included_by_default() {
    // By default, ALL fields are included in signatures
    // Events with different field values are NOT deduplicated
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
        // Each user gets their own signature - NOT throttled together
        for user_id in 1..=5 {
            tracing::error!(user_id = user_id, "Failed to fetch user");
        }
    });

    // All 5 events logged - each has a different user_id value
    assert_eq!(
        capture.count(),
        5,
        "Events with different field values should NOT be throttled together"
    );
}

#[test]
fn test_same_field_values_are_throttled() {
    // Events with the SAME field values share the same signature
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
        // Same user_id = same signature = throttled together
        for _ in 0..5 {
            tracing::error!(user_id = 123, "Failed to fetch user");
        }
    });

    // Only 2 events logged - same signature
    assert_eq!(
        capture.count(),
        2,
        "Events with same field values should be throttled together"
    );
}

#[test]
fn test_excluded_fields_not_in_signature() {
    // Exclude request_id so events with different request_id but same user_id are throttled
    let rate_limit = TracingRateLimitLayer::builder()
        .with_policy(Policy::count_based(2).unwrap())
        .with_excluded_fields(vec!["request_id".to_string()])
        .build()
        .unwrap();

    let capture = MockCaptureLayer::new();

    let rate_limit_filter = rate_limit.clone();
    let subscriber = tracing_subscriber::registry()
        .with(rate_limit)
        .with(capture.clone().with_filter(rate_limit_filter));

    tracing::subscriber::with_default(subscriber, || {
        // Same user_id, different request_id
        // request_id is excluded, so these have the SAME signature
        for i in 0..5 {
            tracing::error!(
                user_id = 123,
                request_id = format!("req-{}", i),
                "Failed to fetch user"
            );
        }
    });

    // Only 2 events logged - request_id excluded from signature
    assert_eq!(
        capture.count(),
        2,
        "Excluded fields should not affect signature"
    );
}

#[test]
fn test_multiple_excluded_fields() {
    let rate_limit = TracingRateLimitLayer::builder()
        .with_policy(Policy::count_based(2).unwrap())
        .with_excluded_fields(vec!["request_id".to_string(), "trace_id".to_string()])
        .build()
        .unwrap();

    let capture = MockCaptureLayer::new();

    let rate_limit_filter = rate_limit.clone();
    let subscriber = tracing_subscriber::registry()
        .with(rate_limit)
        .with(capture.clone().with_filter(rate_limit_filter));

    tracing::subscriber::with_default(subscriber, || {
        for i in 0..5 {
            tracing::error!(
                user_id = 123,
                request_id = format!("req-{}", i),
                trace_id = format!("trace-{}", i),
                "Failed to fetch user"
            );
        }
    });

    // Only 2 events - both request_id and trace_id excluded
    assert_eq!(
        capture.count(),
        2,
        "Multiple excluded fields should all be excluded"
    );
}

#[test]
fn test_different_error_codes_not_throttled() {
    // Different error codes = different signatures (by default)
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
        // 3 AUTH_FAILED events
        for _ in 0..3 {
            tracing::error!(error_code = "AUTH_FAILED", "Authentication failed");
        }

        // 3 TIMEOUT events - different signature
        for _ in 0..3 {
            tracing::error!(error_code = "TIMEOUT", "Request timeout");
        }
    });

    // 4 total: 2 AUTH_FAILED + 2 TIMEOUT (independent limits)
    assert_eq!(
        capture.count(),
        4,
        "Different field values create different signatures"
    );
}

#[test]
fn test_no_fields_on_event() {
    // Events without fields still work correctly
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
        for _ in 0..5 {
            tracing::error!("Simple error without fields");
        }
    });

    // Only 2 logged - same signature (no fields)
    assert_eq!(
        capture.count(),
        2,
        "Events without fields share the same signature"
    );
}

#[test]
fn test_combined_span_context_and_event_fields() {
    // Span context fields (opt-in) + event fields (default all) work together
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
        // Alice's AUTH_FAILED errors
        {
            let span = tracing::info_span!("request", user_id = "alice");
            let _enter = span.enter();
            for _ in 0..3 {
                tracing::error!(error_code = "AUTH_FAILED", "Authentication failed");
            }
        }

        // Alice's TIMEOUT errors - different signature (different error_code)
        {
            let span = tracing::info_span!("request", user_id = "alice");
            let _enter = span.enter();
            for _ in 0..3 {
                tracing::error!(error_code = "TIMEOUT", "Request timeout");
            }
        }

        // Bob's AUTH_FAILED errors - different signature (different user_id)
        {
            let span = tracing::info_span!("request", user_id = "bob");
            let _enter = span.enter();
            for _ in 0..3 {
                tracing::error!(error_code = "AUTH_FAILED", "Authentication failed");
            }
        }
    });

    // 6 total: 2 per (user_id, error_code) combination
    assert_eq!(
        capture.count(),
        6,
        "Span context and event fields both contribute to signature"
    );
}

#[test]
fn test_excluded_fields_with_span_context() {
    let rate_limit = TracingRateLimitLayer::builder()
        .with_policy(Policy::count_based(2).unwrap())
        .with_span_context_fields(vec!["user_id".to_string()])
        .with_excluded_fields(vec!["request_id".to_string()])
        .build()
        .unwrap();

    let capture = MockCaptureLayer::new();

    let rate_limit_filter = rate_limit.clone();
    let subscriber = tracing_subscriber::registry()
        .with(rate_limit)
        .with(capture.clone().with_filter(rate_limit_filter));

    tracing::subscriber::with_default(subscriber, || {
        let span = tracing::info_span!("request", user_id = "alice");
        let _enter = span.enter();

        // Same user_id and error_code, different request_id
        // request_id is excluded, so same signature
        for i in 0..5 {
            tracing::error!(
                error_code = "AUTH_FAILED",
                request_id = format!("req-{}", i),
                "Authentication failed"
            );
        }
    });

    // Only 2 logged - request_id excluded
    assert_eq!(capture.count(), 2, "Excluded fields work with span context");
}

#[test]
fn test_excluded_field_not_present() {
    // Test that excluding a field that isn't present doesn't cause issues
    // Events should still be throttled correctly based on fields that ARE present
    let rate_limit = TracingRateLimitLayer::builder()
        .with_policy(Policy::count_based(2).unwrap())
        .with_excluded_fields(vec!["request_id".to_string(), "trace_id".to_string()])
        .build()
        .unwrap();

    let capture = MockCaptureLayer::new();

    let rate_limit_filter = rate_limit.clone();
    let subscriber = tracing_subscriber::registry()
        .with(rate_limit)
        .with(capture.clone().with_filter(rate_limit_filter));

    tracing::subscriber::with_default(subscriber, || {
        // Events with same user_id, no request_id or trace_id present
        // Should be throttled together (same signature)
        for _ in 0..5 {
            tracing::error!(user_id = 123, "Failed to fetch user");
        }

        // Events with different user_id, still no request_id or trace_id
        // Should have different signature (different user_id)
        for _ in 0..5 {
            tracing::error!(user_id = 456, "Failed to fetch user");
        }
    });

    // 4 total: 2 for user_id=123, 2 for user_id=456
    // Excluded fields not being present should not affect throttling
    assert_eq!(
        capture.count(),
        4,
        "Excluded fields that aren't present should not affect signature"
    );
}
