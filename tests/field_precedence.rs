//! Tests for field precedence and collision behavior.
//!
//! These tests verify how the rate limiter handles scenarios where
//! field names collide between span context and event fields, and
//! how different rate limiting policies work with field-based signatures.

use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::Layer;
use tracing_throttle::infrastructure::mocks::MockCaptureLayer;
use tracing_throttle::{Policy, TracingRateLimitLayer};

#[test]
fn test_event_field_overrides_span_context() {
    // When the same field name exists in both span context and event fields,
    // the event field value should take precedence
    let rate_limit = TracingRateLimitLayer::builder()
        .with_policy(Policy::count_based(2).unwrap())
        .with_span_context_fields(vec!["user_id".to_string()])
        // Event fields are included by default, no need to specify
        .build()
        .unwrap();

    let capture = MockCaptureLayer::new();

    let rate_limit_filter = rate_limit.clone();
    let subscriber = tracing_subscriber::registry()
        .with(rate_limit)
        .with(capture.clone().with_filter(rate_limit_filter));

    tracing::subscriber::with_default(subscriber, || {
        // Span has user_id = "alice"
        let span = tracing::info_span!("request", user_id = "alice");
        let _enter = span.enter();

        // Events override with different user_id values
        for _ in 0..3 {
            tracing::info!(user_id = "bob", "Event with bob");
        }

        for _ in 0..3 {
            tracing::info!(user_id = "charlie", "Event with charlie");
        }
    });

    // Should have 4 total: 2 for bob + 2 for charlie
    // If span context won, we'd only have 2 (all as alice)
    assert_eq!(
        capture.count(),
        4,
        "Event fields should override span context fields"
    );
}

#[test]
fn test_token_bucket_policy_with_span_fields() {
    let rate_limit = TracingRateLimitLayer::builder()
        .with_policy(Policy::token_bucket(2.0, 100.0).unwrap()) // 2 capacity, 100/sec refill
        .with_span_context_fields(vec!["user_id".to_string()])
        .build()
        .unwrap();

    let capture = MockCaptureLayer::new();

    let rate_limit_filter = rate_limit.clone();
    let subscriber = tracing_subscriber::registry()
        .with(rate_limit)
        .with(capture.clone().with_filter(rate_limit_filter));

    tracing::subscriber::with_default(subscriber, || {
        // Alice's events - should allow 2 immediately (burst capacity)
        {
            let span = tracing::info_span!("request", user_id = "alice");
            let _enter = span.enter();
            for _ in 0..4 {
                tracing::info!("Event");
            }
        }

        // Bob's events - should also allow 2 (independent bucket)
        {
            let span = tracing::info_span!("request", user_id = "bob");
            let _enter = span.enter();
            for _ in 0..4 {
                tracing::info!("Event");
            }
        }
    });

    // Should have 4 total: 2 for alice + 2 for bob
    assert_eq!(
        capture.count(),
        4,
        "Token bucket should work independently per span context"
    );
}

#[test]
fn test_token_bucket_policy_with_event_fields() {
    let rate_limit = TracingRateLimitLayer::builder()
        .with_policy(Policy::token_bucket(2.0, 100.0).unwrap())
        .build()
        .unwrap();

    let capture = MockCaptureLayer::new();

    let rate_limit_filter = rate_limit.clone();
    let subscriber = tracing_subscriber::registry()
        .with(rate_limit)
        .with(capture.clone().with_filter(rate_limit_filter));

    tracing::subscriber::with_default(subscriber, || {
        // AUTH_FAILED errors - should allow 2
        for _ in 0..4 {
            tracing::error!(error_code = "AUTH_FAILED", "Error");
        }

        // TIMEOUT errors - should also allow 2
        for _ in 0..4 {
            tracing::error!(error_code = "TIMEOUT", "Error");
        }
    });

    assert_eq!(
        capture.count(),
        4,
        "Token bucket should work independently per event field"
    );
}

#[test]
fn test_time_window_policy_with_span_fields() {
    use std::time::Duration;

    let rate_limit = TracingRateLimitLayer::builder()
        .with_policy(Policy::time_window(1, Duration::from_secs(1)).unwrap())
        .with_span_context_fields(vec!["user_id".to_string()])
        .build()
        .unwrap();

    let capture = MockCaptureLayer::new();

    let rate_limit_filter = rate_limit.clone();
    let subscriber = tracing_subscriber::registry()
        .with(rate_limit)
        .with(capture.clone().with_filter(rate_limit_filter));

    tracing::subscriber::with_default(subscriber, || {
        // Alice's events - should allow 1 per time window
        {
            let span = tracing::info_span!("request", user_id = "alice");
            let _enter = span.enter();
            for _ in 0..3 {
                tracing::info!("Event");
            }
        }

        // Bob's events - should also allow 1 (independent window)
        {
            let span = tracing::info_span!("request", user_id = "bob");
            let _enter = span.enter();
            for _ in 0..3 {
                tracing::info!("Event");
            }
        }
    });

    // Should have 2 total: 1 for alice + 1 for bob
    assert_eq!(
        capture.count(),
        2,
        "Time window should work independently per span context"
    );
}

#[test]
fn test_exponential_backoff_with_event_fields() {
    let rate_limit = TracingRateLimitLayer::builder()
        .with_policy(Policy::exponential_backoff())
        .build()
        .unwrap();

    let capture = MockCaptureLayer::new();

    let rate_limit_filter = rate_limit.clone();
    let subscriber = tracing_subscriber::registry()
        .with(rate_limit)
        .with(capture.clone().with_filter(rate_limit_filter));

    tracing::subscriber::with_default(subscriber, || {
        // AUTH_FAILED errors - should allow 1st, 2nd, suppress 3rd, allow 4th = 3 total
        for _ in 0..4 {
            tracing::error!(error_code = "AUTH_FAILED", "Error");
        }

        // TIMEOUT errors - should also follow same pattern (independent backoff)
        for _ in 0..4 {
            tracing::error!(error_code = "TIMEOUT", "Error");
        }
    });

    // Should have 6 total: 3 for AUTH_FAILED + 3 for TIMEOUT
    // (exponential backoff allows: 1st, 2nd, 4th events)
    assert_eq!(
        capture.count(),
        6,
        "Exponential backoff should work independently per event field"
    );
}

#[test]
fn test_deeply_nested_spans() {
    // Test that fields are correctly inherited through 3+ levels of nesting
    let rate_limit = TracingRateLimitLayer::builder()
        .with_policy(Policy::count_based(2).unwrap())
        .with_span_context_fields(vec!["request_id".to_string(), "user_id".to_string()])
        .build()
        .unwrap();

    let capture = MockCaptureLayer::new();

    let rate_limit_filter = rate_limit.clone();
    let subscriber = tracing_subscriber::registry()
        .with(rate_limit)
        .with(capture.clone().with_filter(rate_limit_filter));

    tracing::subscriber::with_default(subscriber, || {
        let request_span = tracing::info_span!("request", request_id = "req-123");
        let _request_enter = request_span.enter();

        {
            let auth_span = tracing::info_span!("auth", user_id = "alice");
            let _auth_enter = auth_span.enter();

            {
                let handler_span = tracing::info_span!("handler");
                let _handler_enter = handler_span.enter();

                // This event should inherit both request_id and user_id
                for _ in 0..3 {
                    tracing::info!("Deep event");
                }
            }
        }

        {
            let auth_span = tracing::info_span!("auth", user_id = "bob");
            let _auth_enter = auth_span.enter();

            {
                let handler_span = tracing::info_span!("handler");
                let _handler_enter = handler_span.enter();

                // This should have different signature due to different user_id
                for _ in 0..3 {
                    tracing::info!("Deep event");
                }
            }
        }
    });

    // Should have 4 total: 2 for (req-123, alice) + 2 for (req-123, bob)
    assert_eq!(
        capture.count(),
        4,
        "Should correctly inherit fields through deeply nested spans"
    );
}

#[test]
fn test_span_field_shadowing() {
    // Test that child span fields override parent span fields
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
        let parent_span = tracing::info_span!("parent", user_id = "alice");
        let _parent_enter = parent_span.enter();

        // Event in parent span - should use "alice"
        for _ in 0..3 {
            tracing::info!("Parent event");
        }

        {
            // Child span overrides user_id with "bob"
            let child_span = tracing::info_span!("child", user_id = "bob");
            let _child_enter = child_span.enter();

            // Event in child span - should use "bob" (child overrides parent)
            for _ in 0..3 {
                tracing::info!("Child event");
            }
        }
    });

    // Should have 4 total: 2 for alice + 2 for bob
    assert_eq!(
        capture.count(),
        4,
        "Child span fields should override parent span fields"
    );
}

#[test]
fn test_different_field_value_types() {
    // Test that various field types (u64, i64, f64, bool, str) work correctly
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
        // String value
        for _ in 0..3 {
            tracing::info!(value = "string", "Event");
        }

        // u64 value
        for _ in 0..3 {
            tracing::info!(value = 123u64, "Event");
        }

        // i64 value
        for _ in 0..3 {
            tracing::info!(value = -456i64, "Event");
        }

        // f64 value
        for _ in 0..3 {
            tracing::info!(value = 123.456f64, "Event");
        }

        // bool value
        for _ in 0..3 {
            tracing::info!(value = true, "Event");
        }
    });

    // Should have 10 total: 2 per value type
    assert_eq!(
        capture.count(),
        10,
        "Different field value types should create different signatures"
    );
}

#[test]
fn test_empty_string_field_values() {
    // Test that empty string values are treated as valid field values
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
        // Empty string category
        for _ in 0..3 {
            tracing::info!(category = "", "Event");
        }

        // Non-empty category
        for _ in 0..3 {
            tracing::info!(category = "error", "Event");
        }
    });

    // Should have 4 total: 2 for empty string + 2 for "error"
    assert_eq!(
        capture.count(),
        4,
        "Empty string field values should be treated as distinct"
    );
}

#[test]
fn test_special_characters_in_field_values() {
    // Test that special characters in field values are handled correctly
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
        // Path with special characters
        for _ in 0..3 {
            tracing::info!(path = "/api/users/{id}/posts", "Event");
        }

        // Path with Unicode
        for _ in 0..3 {
            tracing::info!(path = "/api/用户/posts", "Event");
        }

        // Path with newlines
        for _ in 0..3 {
            tracing::info!(path = "/api\nusers", "Event");
        }
    });

    // Should have 6 total: 2 per unique path
    assert_eq!(
        capture.count(),
        6,
        "Special characters in field values should work correctly"
    );
}
