//! Integration tests for exempt targets functionality.

use tracing::info;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::Layer;
use tracing_throttle::{Policy, TracingRateLimitLayer};

#[test]
fn test_exempt_targets_never_throttled() {
    let layer = TracingRateLimitLayer::builder()
        .with_policy(Policy::count_based(2).unwrap())
        .with_exempt_targets(vec![
            "security::audit".to_string(),
            "compliance::logging".to_string(),
        ])
        .build()
        .unwrap();

    let subscriber = tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_filter(layer.clone()));

    tracing::subscriber::with_default(subscriber, || {
        // Regular events - throttled after 2
        for _ in 0..5 {
            info!("Normal event");
        }

        // Security events - never throttled
        for _ in 0..10 {
            info!(target: "security::audit", "Security event");
        }

        // Compliance events - never throttled
        for _ in 0..10 {
            info!(target: "compliance::logging", "Compliance event");
        }
    });

    let metrics = layer.metrics();

    // 2 normal events + 10 security + 10 compliance = 22 allowed
    assert_eq!(metrics.events_allowed(), 22);

    // 3 normal events suppressed
    assert_eq!(metrics.events_suppressed(), 3);
}

#[test]
fn test_exempt_targets_mixed_with_regular() {
    let layer = TracingRateLimitLayer::builder()
        .with_policy(Policy::count_based(3).unwrap())
        .with_exempt_targets(vec!["critical".to_string()])
        .build()
        .unwrap();

    let subscriber = tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_filter(layer.clone()));

    tracing::subscriber::with_default(subscriber, || {
        // Interleave exempt and regular events
        for i in 0..10 {
            if i % 2 == 0 {
                info!(target: "critical", "Critical event");
            } else {
                info!("Regular event");
            }
        }
    });

    let metrics = layer.metrics();

    // 5 critical (all allowed) + 3 regular (first 3 allowed) = 8 allowed
    assert_eq!(metrics.events_allowed(), 8);

    // 2 regular events suppressed
    assert_eq!(metrics.events_suppressed(), 2);
}

#[test]
fn test_empty_exempt_targets() {
    let layer = TracingRateLimitLayer::builder()
        .with_policy(Policy::count_based(2).unwrap())
        .with_exempt_targets(vec![])
        .build()
        .unwrap();

    let subscriber = tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_filter(layer.clone()));

    tracing::subscriber::with_default(subscriber, || {
        for _ in 0..5 {
            info!("Event");
        }
    });

    let metrics = layer.metrics();

    // No exemptions, normal throttling applies
    assert_eq!(metrics.events_allowed(), 2);
    assert_eq!(metrics.events_suppressed(), 3);
}

#[test]
fn test_exempt_targets_with_different_levels() {
    let layer = TracingRateLimitLayer::builder()
        .with_policy(Policy::count_based(1).unwrap())
        .with_exempt_targets(vec!["audit".to_string()])
        .build()
        .unwrap();

    let subscriber = tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_filter(layer.clone()));

    tracing::subscriber::with_default(subscriber, || {
        // Exempt target with different levels
        for _ in 0..3 {
            tracing::info!(target: "audit", "Audit info");
        }
        for _ in 0..3 {
            tracing::warn!(target: "audit", "Audit warning");
        }
        for _ in 0..3 {
            tracing::error!(target: "audit", "Audit error");
        }
    });

    let metrics = layer.metrics();

    // All 9 audit events allowed (3 info + 3 warn + 3 error)
    assert_eq!(metrics.events_allowed(), 9);
    assert_eq!(metrics.events_suppressed(), 0);
}
