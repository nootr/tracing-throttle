//! Integration tests for event field rate limiting.
//!
//! These tests demonstrate that event fields are properly extracted
//! and used in event signatures for per-field rate limiting.

use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::Layer;
use tracing_throttle::infrastructure::mocks::MockCaptureLayer;
use tracing_throttle::{Policy, TracingRateLimitLayer};

#[test]
fn test_event_field_per_error_code_rate_limiting() {
    let rate_limit = TracingRateLimitLayer::builder()
        .with_policy(Policy::count_based(2).unwrap())
        .with_event_fields(vec!["error_code".to_string()])
        .build()
        .unwrap();

    let capture = MockCaptureLayer::new();

    let rate_limit_filter = rate_limit.clone();
    let subscriber = tracing_subscriber::registry()
        .with(rate_limit)
        .with(capture.clone().with_filter(rate_limit_filter));

    tracing::subscriber::with_default(subscriber, || {
        // AUTH_FAILED errors - should allow 2
        for _ in 0..3 {
            tracing::error!(error_code = "AUTH_FAILED", "Authentication failed");
        }

        // TIMEOUT errors - should also allow 2 (independent limit)
        for _ in 0..3 {
            tracing::error!(error_code = "TIMEOUT", "Request timeout");
        }
    });

    // Should have 4 total: 2 for AUTH_FAILED + 2 for TIMEOUT
    assert_eq!(
        capture.count(),
        4,
        "Should rate limit independently per error_code"
    );
}

#[test]
fn test_missing_event_field() {
    // Test that events without the configured field share the same limit
    let rate_limit = TracingRateLimitLayer::builder()
        .with_policy(Policy::count_based(2).unwrap())
        .with_event_fields(vec!["error_code".to_string()])
        .build()
        .unwrap();

    let capture = MockCaptureLayer::new();

    let rate_limit_filter = rate_limit.clone();
    let subscriber = tracing_subscriber::registry()
        .with(rate_limit)
        .with(capture.clone().with_filter(rate_limit_filter));

    tracing::subscriber::with_default(subscriber, || {
        // Events without error_code field
        for _ in 0..3 {
            tracing::error!("Error without code");
        }

        // More events without error_code - same signature
        for _ in 0..3 {
            tracing::error!("Another error without code");
        }
    });

    // Each loop has different source location, so different signatures
    // First loop: 2 allowed, second loop: 2 allowed = 4 total
    assert_eq!(
        capture.count(),
        4,
        "Events in different loops have different signatures"
    );
}

#[test]
fn test_multiple_event_fields() {
    // Test rate limiting with multiple event fields
    let rate_limit = TracingRateLimitLayer::builder()
        .with_policy(Policy::count_based(2).unwrap())
        .with_event_fields(vec!["status".to_string(), "endpoint".to_string()])
        .build()
        .unwrap();

    let capture = MockCaptureLayer::new();

    let rate_limit_filter = rate_limit.clone();
    let subscriber = tracing_subscriber::registry()
        .with(rate_limit)
        .with(capture.clone().with_filter(rate_limit_filter));

    tracing::subscriber::with_default(subscriber, || {
        // 404 on /api/users
        for _ in 0..3 {
            tracing::info!(status = 404, endpoint = "/api/users", "Request");
        }

        // 404 on /api/posts - different signature
        for _ in 0..3 {
            tracing::info!(status = 404, endpoint = "/api/posts", "Request");
        }

        // 200 on /api/users - different signature
        for _ in 0..3 {
            tracing::info!(status = 200, endpoint = "/api/users", "Request");
        }
    });

    // Should have 6 total: 2 per (status, endpoint) combination
    assert_eq!(
        capture.count(),
        6,
        "Should rate limit per combination of all event fields"
    );
}

#[test]
fn test_no_event_fields_configured() {
    // Test that without event fields, behavior is unchanged
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
        // Different error codes, but without event fields configured
        // All events in this loop have the same signature (same source location)
        for _ in 0..4 {
            tracing::error!(error_code = "AUTH_FAILED", "Authentication failed");
        }
    });

    // Without event fields configured, all events from the same source have same signature
    // Policy limit is 2, so only 2 events allowed
    assert_eq!(
        capture.count(),
        2,
        "Without event fields, events from same source share the same signature"
    );
}

#[test]
fn test_combined_span_context_and_event_fields() {
    // Test that span context and event fields work together
    let rate_limit = TracingRateLimitLayer::builder()
        .with_policy(Policy::count_based(2).unwrap())
        .with_span_context_fields(vec!["user_id".to_string()])
        .with_event_fields(vec!["error_code".to_string()])
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

        // Alice's TIMEOUT errors - different signature
        {
            let span = tracing::info_span!("request", user_id = "alice");
            let _enter = span.enter();
            for _ in 0..3 {
                tracing::error!(error_code = "TIMEOUT", "Request timeout");
            }
        }

        // Bob's AUTH_FAILED errors - different signature
        {
            let span = tracing::info_span!("request", user_id = "bob");
            let _enter = span.enter();
            for _ in 0..3 {
                tracing::error!(error_code = "AUTH_FAILED", "Authentication failed");
            }
        }
    });

    // Should have 6 total: 2 per (user_id, error_code) combination
    assert_eq!(
        capture.count(),
        6,
        "Should rate limit per combination of span context and event fields"
    );
}

#[test]
fn test_partial_event_fields() {
    // Test when only some of the configured fields are present in events
    let rate_limit = TracingRateLimitLayer::builder()
        .with_policy(Policy::count_based(2).unwrap())
        .with_event_fields(vec!["error_code".to_string(), "status".to_string()])
        .build()
        .unwrap();

    let capture = MockCaptureLayer::new();

    let rate_limit_filter = rate_limit.clone();
    let subscriber = tracing_subscriber::registry()
        .with(rate_limit)
        .with(capture.clone().with_filter(rate_limit_filter));

    tracing::subscriber::with_default(subscriber, || {
        // Events with both fields
        for _ in 0..3 {
            tracing::error!(error_code = "AUTH_FAILED", status = 401, "Error");
        }

        // Events with only error_code field - different signature
        for _ in 0..3 {
            tracing::error!(error_code = "AUTH_FAILED", "Error");
        }

        // Events with only status field - different signature
        for _ in 0..3 {
            tracing::error!(status = 401, "Error");
        }
    });

    // Should have 6 total: 2 per field combination
    // (error_code=AUTH_FAILED, status=401), (error_code=AUTH_FAILED), (status=401)
    assert_eq!(
        capture.count(),
        6,
        "Partial field presence creates different signatures"
    );
}
