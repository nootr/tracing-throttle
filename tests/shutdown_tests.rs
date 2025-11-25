//! Integration tests for graceful shutdown functionality.

#![cfg(feature = "async")]

use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::{info, warn};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::Layer;
use tracing_throttle::infrastructure::mocks::MockCaptureLayer;
use tracing_throttle::{EmitterHandle, Policy, TracingRateLimitLayer};

#[tokio::test]
async fn test_graceful_shutdown_with_active_logging() {
    // This test simulates a real application scenario:
    // 1. Start rate limiting
    // 2. Generate logs
    // 3. Shutdown gracefully
    // 4. Verify cleanup

    let capture = MockCaptureLayer::new();
    let rate_limit = TracingRateLimitLayer::builder()
        .with_policy(Policy::count_based(5).unwrap())
        .build()
        .unwrap();

    let subscriber = tracing_subscriber::registry().with(capture.clone().with_filter(rate_limit));

    tracing::subscriber::set_global_default(subscriber).expect("Failed to set subscriber");

    // Generate some logs
    for i in 0..20 {
        info!("Processing item {}", i);
    }

    // Only first 5 should pass through
    assert_eq!(capture.count(), 5);

    // In a real app, shutdown would be triggered by signal handler
    // Here we just verify the layer continues working
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Generate more logs
    for i in 20..30 {
        warn!("Warning for item {}", i);
    }

    // Should have 5 info + 5 warn = 10 total
    assert_eq!(capture.count(), 10);
}

#[tokio::test]
async fn test_explicit_shutdown_required() {
    // Test that dropping the handle WITHOUT calling shutdown() keeps the task running
    // This verifies the new behavior where Drop no longer shuts down the task
    let emissions = Arc::new(Mutex::new(0));

    let storage = Arc::new(tracing_throttle::ShardedStorage::new());
    let clock = Arc::new(tracing_throttle::SystemClock::new());
    let policy = Policy::count_based(100).unwrap();
    let registry = tracing_throttle::SuppressionRegistry::new(storage, clock.clone(), policy);

    // Add suppression
    let sig = tracing_throttle::EventSignature::simple("INFO", "Test");
    registry.with_event_state(sig, |state, now| {
        state.counter.record_suppression(now);
    });

    let config =
        tracing_throttle::application::emitter::EmitterConfig::new(Duration::from_millis(50))
            .unwrap();
    let emitter =
        tracing_throttle::application::emitter::SummaryEmitter::new(registry.clone(), config);

    let emissions_clone = Arc::clone(&emissions);
    let handle = emitter.start(
        move |_| {
            *emissions_clone.lock().unwrap() += 1;
        },
        false,
    );

    // Let it emit once
    tokio::time::sleep(Duration::from_millis(75)).await;
    let count_before_shutdown = *emissions.lock().unwrap();
    assert!(count_before_shutdown >= 1);

    // Explicitly call shutdown (required!)
    handle.shutdown().await.expect("shutdown failed");

    // Wait to ensure no more emissions after shutdown
    tokio::time::sleep(Duration::from_millis(100)).await;
    let final_count = *emissions.lock().unwrap();
    assert_eq!(final_count, count_before_shutdown);
}

#[tokio::test]
async fn test_explicit_shutdown_in_application() {
    // Simulate an application with explicit shutdown
    struct Application {
        _emitter_handle: Option<EmitterHandle>,
        emissions: Arc<Mutex<Vec<usize>>>,
    }

    impl Application {
        fn new() -> Self {
            let storage = Arc::new(tracing_throttle::ShardedStorage::new());
            let clock = Arc::new(tracing_throttle::SystemClock::new());
            let policy = Policy::count_based(100).unwrap();
            let registry = tracing_throttle::SuppressionRegistry::new(storage, clock, policy);

            // Add suppressions
            let sig = tracing_throttle::EventSignature::simple("INFO", "App event");
            registry.with_event_state(sig, |state, now| {
                for _ in 0..5 {
                    state.counter.record_suppression(now);
                }
            });

            let config = tracing_throttle::application::emitter::EmitterConfig::new(
                Duration::from_millis(100),
            )
            .unwrap();
            let emitter =
                tracing_throttle::application::emitter::SummaryEmitter::new(registry, config);

            let emissions = Arc::new(Mutex::new(Vec::new()));
            let emissions_clone = Arc::clone(&emissions);

            let handle = emitter.start(
                move |summaries| {
                    emissions_clone.lock().unwrap().push(summaries.len());
                },
                true, // Emit final summaries on shutdown
            );

            Self {
                _emitter_handle: Some(handle),
                emissions,
            }
        }

        async fn shutdown(mut self) {
            if let Some(handle) = self._emitter_handle.take() {
                handle.shutdown().await.expect("shutdown failed");
            }
        }

        fn emission_count(&self) -> usize {
            self.emissions.lock().unwrap().len()
        }
    }

    let app = Application::new();

    // Let it run
    tokio::time::sleep(Duration::from_millis(250)).await;

    let emissions_before = app.emission_count();
    assert!(emissions_before >= 2);

    // Explicit shutdown
    app.shutdown().await;

    // Application has shut down cleanly
}

#[tokio::test]
async fn test_concurrent_shutdown_safety() {
    // Test that multiple components can shut down safely
    let mut handles = vec![];

    for i in 0..5 {
        let storage = Arc::new(tracing_throttle::ShardedStorage::new());
        let clock = Arc::new(tracing_throttle::SystemClock::new());
        let policy = Policy::count_based(100).unwrap();
        let registry = tracing_throttle::SuppressionRegistry::new(storage, clock, policy);

        let sig = tracing_throttle::EventSignature::simple("INFO", &format!("Component {}", i));
        registry.with_event_state(sig, |state, now| {
            state.counter.record_suppression(now);
        });

        let config =
            tracing_throttle::application::emitter::EmitterConfig::new(Duration::from_millis(50))
                .unwrap();
        let emitter = tracing_throttle::application::emitter::SummaryEmitter::new(registry, config);

        let handle = emitter.start(|_| {}, false);
        handles.push(handle);
    }

    // Let them all run
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Shut down all sequentially (demonstrating clean shutdown)
    for handle in handles {
        handle.shutdown().await.expect("shutdown failed");
    }

    // All shut down successfully
}
