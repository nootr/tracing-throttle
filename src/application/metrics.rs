//! Observability metrics for rate limiting.
//!
//! Provides metrics about rate limiting behavior for monitoring and debugging.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Metrics tracking rate limiting statistics.
///
/// All metrics use atomic operations for thread-safe updates and reads.
/// Metrics are collected throughout the rate limiting process and can be
/// queried at any time for observability.
#[derive(Debug, Clone)]
pub struct Metrics {
    inner: Arc<MetricsInner>,
}

#[derive(Debug)]
struct MetricsInner {
    /// Total number of events allowed through
    events_allowed: AtomicU64,
    /// Total number of events suppressed
    events_suppressed: AtomicU64,
    /// Total number of signatures evicted from storage
    signatures_evicted: AtomicU64,
}

impl Metrics {
    /// Create a new metrics tracker.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(MetricsInner {
                events_allowed: AtomicU64::new(0),
                events_suppressed: AtomicU64::new(0),
                signatures_evicted: AtomicU64::new(0),
            }),
        }
    }

    /// Record an allowed event.
    pub(crate) fn record_allowed(&self) {
        self.inner.events_allowed.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a suppressed event.
    pub(crate) fn record_suppressed(&self) {
        self.inner.events_suppressed.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a signature eviction.
    pub(crate) fn record_eviction(&self) {
        self.inner
            .signatures_evicted
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Get the total number of events allowed.
    pub fn events_allowed(&self) -> u64 {
        self.inner.events_allowed.load(Ordering::Relaxed)
    }

    /// Get the total number of events suppressed.
    pub fn events_suppressed(&self) -> u64 {
        self.inner.events_suppressed.load(Ordering::Relaxed)
    }

    /// Get the total number of signatures evicted.
    pub fn signatures_evicted(&self) -> u64 {
        self.inner.signatures_evicted.load(Ordering::Relaxed)
    }

    /// Get a snapshot of all metrics.
    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            events_allowed: self.events_allowed(),
            events_suppressed: self.events_suppressed(),
            signatures_evicted: self.signatures_evicted(),
        }
    }

    /// Reset all metrics to zero.
    ///
    /// Useful for testing or when starting a new monitoring period.
    pub fn reset(&self) {
        self.inner.events_allowed.store(0, Ordering::Relaxed);
        self.inner.events_suppressed.store(0, Ordering::Relaxed);
        self.inner.signatures_evicted.store(0, Ordering::Relaxed);
    }
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

/// A point-in-time snapshot of metrics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MetricsSnapshot {
    /// Total number of events allowed through
    pub events_allowed: u64,
    /// Total number of events suppressed
    pub events_suppressed: u64,
    /// Total number of signatures evicted from storage
    pub signatures_evicted: u64,
}

impl MetricsSnapshot {
    /// Calculate the suppression rate (0.0 to 1.0).
    ///
    /// Returns the ratio of suppressed events to total events.
    /// Returns 0.0 if no events have been processed.
    pub fn suppression_rate(&self) -> f64 {
        let total = self.events_allowed.saturating_add(self.events_suppressed);
        if total == 0 {
            0.0
        } else {
            self.events_suppressed as f64 / total as f64
        }
    }

    /// Get the total number of events processed (allowed + suppressed).
    pub fn total_events(&self) -> u64 {
        self.events_allowed.saturating_add(self.events_suppressed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_initial_state() {
        let metrics = Metrics::new();
        assert_eq!(metrics.events_allowed(), 0);
        assert_eq!(metrics.events_suppressed(), 0);
        assert_eq!(metrics.signatures_evicted(), 0);
    }

    #[test]
    fn test_record_allowed() {
        let metrics = Metrics::new();
        metrics.record_allowed();
        metrics.record_allowed();
        metrics.record_allowed();
        assert_eq!(metrics.events_allowed(), 3);
        assert_eq!(metrics.events_suppressed(), 0);
    }

    #[test]
    fn test_record_suppressed() {
        let metrics = Metrics::new();
        metrics.record_suppressed();
        metrics.record_suppressed();
        assert_eq!(metrics.events_allowed(), 0);
        assert_eq!(metrics.events_suppressed(), 2);
    }

    #[test]
    fn test_record_eviction() {
        let metrics = Metrics::new();
        metrics.record_eviction();
        assert_eq!(metrics.signatures_evicted(), 1);
    }

    #[test]
    fn test_snapshot() {
        let metrics = Metrics::new();
        metrics.record_allowed();
        metrics.record_allowed();
        metrics.record_suppressed();
        metrics.record_eviction();

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.events_allowed, 2);
        assert_eq!(snapshot.events_suppressed, 1);
        assert_eq!(snapshot.signatures_evicted, 1);
    }

    #[test]
    fn test_snapshot_suppression_rate() {
        let metrics = Metrics::new();

        // No events - rate should be 0
        assert_eq!(metrics.snapshot().suppression_rate(), 0.0);

        // 1 allowed, 0 suppressed - rate should be 0
        metrics.record_allowed();
        assert_eq!(metrics.snapshot().suppression_rate(), 0.0);

        // 1 allowed, 1 suppressed - rate should be 0.5
        metrics.record_suppressed();
        assert!((metrics.snapshot().suppression_rate() - 0.5).abs() < f64::EPSILON);

        // 1 allowed, 3 suppressed - rate should be 0.75
        metrics.record_suppressed();
        metrics.record_suppressed();
        assert!((metrics.snapshot().suppression_rate() - 0.75).abs() < f64::EPSILON);
    }

    #[test]
    fn test_snapshot_total_events() {
        let metrics = Metrics::new();
        assert_eq!(metrics.snapshot().total_events(), 0);

        metrics.record_allowed();
        metrics.record_allowed();
        metrics.record_suppressed();
        assert_eq!(metrics.snapshot().total_events(), 3);
    }

    #[test]
    fn test_reset() {
        let metrics = Metrics::new();
        metrics.record_allowed();
        metrics.record_suppressed();
        metrics.record_eviction();

        metrics.reset();
        assert_eq!(metrics.events_allowed(), 0);
        assert_eq!(metrics.events_suppressed(), 0);
        assert_eq!(metrics.signatures_evicted(), 0);
    }

    #[test]
    fn test_metrics_clone() {
        let metrics1 = Metrics::new();
        metrics1.record_allowed();

        let metrics2 = metrics1.clone();
        metrics2.record_allowed();

        // Both should see the same value (shared Arc)
        assert_eq!(metrics1.events_allowed(), 2);
        assert_eq!(metrics2.events_allowed(), 2);
    }

    #[test]
    fn test_concurrent_updates() {
        use std::thread;

        let metrics = Metrics::new();
        let mut handles = vec![];

        // Spawn 10 threads, each recording 100 events
        for _ in 0..10 {
            let m = metrics.clone();
            handles.push(thread::spawn(move || {
                for _ in 0..100 {
                    m.record_allowed();
                    m.record_suppressed();
                }
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        assert_eq!(metrics.events_allowed(), 1000);
        assert_eq!(metrics.events_suppressed(), 1000);
    }
}
