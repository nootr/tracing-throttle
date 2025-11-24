//! Suppression summaries and counters.
//!
//! This module tracks how many events have been suppressed and generates
//! periodic summaries for emission.

use crate::domain::signature::EventSignature;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// Thread-safe counter for tracking suppressed events.
///
/// Uses atomics for lock-free concurrent updates in high-throughput scenarios.
#[derive(Debug)]
pub struct SuppressionCounter {
    /// Total number of times this event was suppressed
    suppressed_count: AtomicUsize,
    /// Timestamp of first suppression (nanoseconds since epoch)
    first_suppressed_nanos: AtomicU64,
    /// Timestamp of last suppression (nanoseconds since epoch)
    last_suppressed_nanos: AtomicU64,
}

impl SuppressionCounter {
    /// Create a new counter with initial suppression.
    pub fn new(initial_timestamp: Instant) -> Self {
        let nanos = Self::instant_to_nanos(initial_timestamp);
        Self {
            suppressed_count: AtomicUsize::new(1),
            first_suppressed_nanos: AtomicU64::new(nanos),
            last_suppressed_nanos: AtomicU64::new(nanos),
        }
    }

    /// Record a new suppression event.
    pub fn record_suppression(&self, timestamp: Instant) {
        self.suppressed_count.fetch_add(1, Ordering::Relaxed);
        let nanos = Self::instant_to_nanos(timestamp);
        self.last_suppressed_nanos.store(nanos, Ordering::Relaxed);
    }

    /// Get the current suppression count.
    pub fn count(&self) -> usize {
        self.suppressed_count.load(Ordering::Relaxed)
    }

    /// Get the timestamp of the first suppression.
    pub fn first_suppressed(&self) -> Instant {
        let nanos = self.first_suppressed_nanos.load(Ordering::Relaxed);
        Self::nanos_to_instant(nanos)
    }

    /// Get the timestamp of the last suppression.
    pub fn last_suppressed(&self) -> Instant {
        let nanos = self.last_suppressed_nanos.load(Ordering::Relaxed);
        Self::nanos_to_instant(nanos)
    }

    /// Reset the counter for a new tracking period.
    pub fn reset(&self, timestamp: Instant) {
        let nanos = Self::instant_to_nanos(timestamp);
        self.suppressed_count.store(0, Ordering::Relaxed);
        self.first_suppressed_nanos.store(nanos, Ordering::Relaxed);
        self.last_suppressed_nanos.store(nanos, Ordering::Relaxed);
    }

    /// Convert Instant to nanoseconds for atomic storage.
    ///
    /// We store relative to a base instant to avoid overflow issues.
    fn instant_to_nanos(instant: Instant) -> u64 {
        // Use a static base instant (process start time approximation)
        static BASE: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        let base = BASE.get_or_init(Instant::now);

        instant
            .saturating_duration_since(*base)
            .as_nanos()
            .try_into()
            .unwrap_or(u64::MAX)
    }

    /// Convert nanoseconds back to Instant.
    fn nanos_to_instant(nanos: u64) -> Instant {
        static BASE: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        let base = BASE.get_or_init(Instant::now);

        *base + Duration::from_nanos(nanos)
    }
}

/// A summary of suppressed events for a particular signature.
///
/// This is emitted periodically to inform about suppression activity.
#[derive(Debug, Clone)]
pub struct SuppressionSummary {
    /// The signature of the suppressed event
    pub signature: EventSignature,
    /// Number of times the event was suppressed
    pub count: usize,
    /// When the first suppression occurred
    pub first_suppressed: Instant,
    /// When the last suppression occurred
    pub last_suppressed: Instant,
    /// Duration of the suppression period
    pub duration: Duration,
}

impl SuppressionSummary {
    /// Create a summary from a counter.
    pub fn from_counter(signature: EventSignature, counter: &SuppressionCounter) -> Self {
        let first = counter.first_suppressed();
        let last = counter.last_suppressed();
        let duration = last.saturating_duration_since(first);

        Self {
            signature,
            count: counter.count(),
            first_suppressed: first,
            last_suppressed: last,
            duration,
        }
    }

    /// Format the summary as a human-readable message.
    pub fn format_message(&self) -> String {
        format!(
            "Event suppressed {} times over {:?} (signature: {})",
            self.count, self.duration, self.signature
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn test_suppression_counter_basic() {
        let now = Instant::now();
        let counter = SuppressionCounter::new(now);

        assert_eq!(counter.count(), 1);

        counter.record_suppression(now);
        assert_eq!(counter.count(), 2);

        counter.record_suppression(now);
        assert_eq!(counter.count(), 3);
    }

    #[test]
    fn test_suppression_counter_timestamps() {
        let start = Instant::now();
        let counter = SuppressionCounter::new(start);

        thread::sleep(Duration::from_millis(10));
        let later = Instant::now();
        counter.record_suppression(later);

        let first = counter.first_suppressed();
        let last = counter.last_suppressed();

        // First should be approximately start
        assert!(first.saturating_duration_since(start) < Duration::from_millis(5));

        // Last should be approximately later
        assert!(last.saturating_duration_since(later) < Duration::from_millis(5));
    }

    #[test]
    fn test_suppression_counter_reset() {
        let now = Instant::now();
        let counter = SuppressionCounter::new(now);

        counter.record_suppression(now);
        counter.record_suppression(now);
        assert_eq!(counter.count(), 3);

        counter.reset(now);
        assert_eq!(counter.count(), 0);
    }

    #[test]
    fn test_suppression_summary_creation() {
        let sig = EventSignature::simple("INFO", "Test message");
        let start = Instant::now();
        let counter = SuppressionCounter::new(start);

        thread::sleep(Duration::from_millis(10));
        counter.record_suppression(Instant::now());

        let summary = SuppressionSummary::from_counter(sig, &counter);

        assert_eq!(summary.signature, sig);
        assert_eq!(summary.count, 2);
        assert!(summary.duration >= Duration::from_millis(10));
    }

    #[test]
    fn test_suppression_summary_message() {
        let sig = EventSignature::simple("INFO", "Test");
        let now = Instant::now();
        let counter = SuppressionCounter::new(now);
        counter.record_suppression(now);

        let summary = SuppressionSummary::from_counter(sig, &counter);
        let message = summary.format_message();

        assert!(message.contains("suppressed 2 times"));
        assert!(message.contains(&sig.to_string()));
    }

    // Edge case tests
    #[test]
    fn test_very_large_suppression_count() {
        let now = Instant::now();
        let counter = SuppressionCounter::new(now);

        // Simulate a very large number of suppressions
        for _ in 0..10_000 {
            counter.record_suppression(now);
        }

        assert_eq!(counter.count(), 10_001); // 10,000 + 1 initial
    }

    #[test]
    fn test_counter_concurrent_updates() {
        use std::sync::Arc;
        use std::thread;

        let now = Instant::now();
        let counter = Arc::new(SuppressionCounter::new(now));
        let mut handles = vec![];

        // Spawn multiple threads updating counter
        for _ in 0..10 {
            let counter_clone = Arc::clone(&counter);
            let handle = thread::spawn(move || {
                for _ in 0..100 {
                    counter_clone.record_suppression(now);
                }
            });
            handles.push(handle);
        }

        for handle in handles {
            handle.join().unwrap();
        }

        // 10 threads * 100 updates + 1 initial = 1001
        assert_eq!(counter.count(), 1001);
    }

    #[test]
    fn test_zero_duration_summary() {
        let sig = EventSignature::simple("INFO", "Test");
        let now = Instant::now();
        let counter = SuppressionCounter::new(now);

        // Immediately create summary (same timestamp)
        let summary = SuppressionSummary::from_counter(sig, &counter);

        assert_eq!(summary.count, 1);
        assert!(summary.duration < Duration::from_millis(1));
    }

    #[test]
    fn test_reset_multiple_times() {
        let now = Instant::now();
        let counter = SuppressionCounter::new(now);

        counter.record_suppression(now);
        assert_eq!(counter.count(), 2);

        counter.reset(now);
        assert_eq!(counter.count(), 0);

        counter.record_suppression(now);
        assert_eq!(counter.count(), 1);

        counter.reset(now);
        assert_eq!(counter.count(), 0);
    }
}
