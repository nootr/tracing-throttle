//! Suppression summaries and counters.
//!
//! This module tracks how many events have been suppressed and generates
//! periodic summaries for emission.

use crate::domain::{metadata::EventMetadata, signature::EventSignature};
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

impl Clone for SuppressionCounter {
    fn clone(&self) -> Self {
        Self {
            suppressed_count: AtomicUsize::new(self.suppressed_count.load(Ordering::Relaxed)),
            first_suppressed_nanos: AtomicU64::new(
                self.first_suppressed_nanos.load(Ordering::Relaxed),
            ),
            last_suppressed_nanos: AtomicU64::new(
                self.last_suppressed_nanos.load(Ordering::Relaxed),
            ),
        }
    }
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

    /// Create a counter from a snapshot (for deserialization).
    ///
    /// This is used by storage backends like Redis to reconstruct state.
    #[cfg(feature = "redis-storage")]
    pub fn from_snapshot(
        suppressed_count: usize,
        first_suppressed: Instant,
        last_suppressed: Instant,
    ) -> Self {
        let first_nanos = Self::instant_to_nanos(first_suppressed);
        let last_nanos = Self::instant_to_nanos(last_suppressed);
        Self {
            suppressed_count: AtomicUsize::new(suppressed_count),
            first_suppressed_nanos: AtomicU64::new(first_nanos),
            last_suppressed_nanos: AtomicU64::new(last_nanos),
        }
    }

    /// Record a new suppression event.
    pub fn record_suppression(&self, timestamp: Instant) {
        // Use AcqRel for fetch_add to synchronize with other threads
        self.suppressed_count.fetch_add(1, Ordering::AcqRel);
        let nanos = Self::instant_to_nanos(timestamp);
        // Use Release to ensure timestamp update is visible
        self.last_suppressed_nanos.store(nanos, Ordering::Release);
    }

    /// Get the current suppression count.
    pub fn count(&self) -> usize {
        // Use Acquire to synchronize with Release/AcqRel operations
        self.suppressed_count.load(Ordering::Acquire)
    }

    /// Get the timestamp of the first suppression.
    pub fn first_suppressed(&self) -> Instant {
        // Use Acquire to synchronize with Release stores
        let nanos = self.first_suppressed_nanos.load(Ordering::Acquire);
        Self::nanos_to_instant(nanos)
    }

    /// Get the timestamp of the last suppression.
    pub fn last_suppressed(&self) -> Instant {
        // Use Acquire to synchronize with Release stores
        let nanos = self.last_suppressed_nanos.load(Ordering::Acquire);
        Self::nanos_to_instant(nanos)
    }

    /// Get a snapshot of the current state (for serialization).
    #[cfg(feature = "redis-storage")]
    pub fn snapshot(&self) -> super::summary::SuppressionSnapshot {
        super::summary::SuppressionSnapshot {
            suppressed_count: self.count(),
            first_suppressed: self.first_suppressed(),
            last_suppressed: self.last_suppressed(),
        }
    }

    /// Reset the counter for a new tracking period.
    ///
    /// # Thread Safety
    ///
    /// Note: This method updates multiple fields independently, so there is no
    /// guarantee that a concurrent reader will see all updates atomically. A reader
    /// could observe the count reset to 0 while timestamps still reflect old values,
    /// or vice versa. This is acceptable in practice since reset is typically called
    /// during initialization or between tracking periods when concurrent access is minimal.
    ///
    /// If you need atomic reset semantics, ensure no concurrent access during reset.
    pub fn reset(&self, timestamp: Instant) {
        let nanos = Self::instant_to_nanos(timestamp);
        // Use Release for visibility
        self.suppressed_count.store(0, Ordering::Release);
        self.first_suppressed_nanos.store(nanos, Ordering::Release);
        self.last_suppressed_nanos.store(nanos, Ordering::Release);
    }

    /// Get the shared base instant for timestamp calculations.
    ///
    /// This ensures instant_to_nanos and nanos_to_instant use the same reference point.
    fn base_instant() -> &'static Instant {
        static BASE: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        BASE.get_or_init(Instant::now)
    }

    /// Convert Instant to nanoseconds for atomic storage.
    ///
    /// We store relative to a base instant to avoid overflow issues.
    ///
    /// # Overflow Handling
    ///
    /// If the duration exceeds u64::MAX nanoseconds (~584 years), it saturates
    /// at u64::MAX. This is handled gracefully in nanos_to_instant().
    fn instant_to_nanos(instant: Instant) -> u64 {
        let base = Self::base_instant();
        instant
            .saturating_duration_since(*base)
            .as_nanos()
            .min(u64::MAX as u128) as u64
    }

    /// Convert nanoseconds back to Instant.
    ///
    /// # Overflow Handling
    ///
    /// If adding the duration would overflow Instant (practically impossible - requires
    /// ~584 years of uptime), returns the base instant. This ensures timestamps never
    /// panic even in extreme edge cases.
    fn nanos_to_instant(nanos: u64) -> Instant {
        let base = Self::base_instant();
        base.checked_add(Duration::from_nanos(nanos))
            .unwrap_or(*base)
    }
}

/// A snapshot of suppression counter state (for serialization).
#[cfg(feature = "redis-storage")]
#[derive(Debug, Clone)]
pub struct SuppressionSnapshot {
    pub suppressed_count: usize,
    pub first_suppressed: Instant,
    pub last_suppressed: Instant,
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
    /// Metadata about the event (for human-readable display)
    pub metadata: Option<EventMetadata>,
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
            metadata: None,
        }
    }

    /// Create a summary from a counter with metadata.
    pub fn from_counter_with_metadata(
        signature: EventSignature,
        counter: &SuppressionCounter,
        metadata: Option<EventMetadata>,
    ) -> Self {
        let first = counter.first_suppressed();
        let last = counter.last_suppressed();
        let duration = last.saturating_duration_since(first);

        Self {
            signature,
            count: counter.count(),
            first_suppressed: first,
            last_suppressed: last,
            duration,
            metadata,
        }
    }

    /// Format the summary as a human-readable message.
    ///
    /// If metadata is available, includes event details.
    /// Otherwise, shows just the signature hash.
    pub fn format_message(&self) -> String {
        if let Some(ref metadata) = self.metadata {
            format!(
                "Suppressed {} times over {:.2}s: {}",
                self.count,
                self.duration.as_secs_f64(),
                metadata.format_brief()
            )
        } else {
            format!(
                "Event suppressed {} times over {:?} (signature: {})",
                self.count, self.duration, self.signature
            )
        }
    }

    /// Format the summary with detailed field information.
    pub fn format_detailed(&self) -> String {
        if let Some(ref metadata) = self.metadata {
            format!(
                "Suppressed {} times over {:.2}s: {}",
                self.count,
                self.duration.as_secs_f64(),
                metadata.format_detailed()
            )
        } else {
            self.format_message()
        }
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

    // === Edge Case and Overflow Tests ===

    #[test]
    fn test_clone_preserves_state() {
        let now = Instant::now();
        let counter = SuppressionCounter::new(now);

        counter.record_suppression(now);
        counter.record_suppression(now);

        let cloned = counter.clone();

        assert_eq!(counter.count(), cloned.count());
        assert_eq!(counter.first_suppressed(), cloned.first_suppressed());
        assert_eq!(counter.last_suppressed(), cloned.last_suppressed());
    }

    #[test]
    fn test_clone_independence() {
        let now = Instant::now();
        let counter1 = SuppressionCounter::new(now);
        let counter2 = counter1.clone();

        // Modify counter1
        counter1.record_suppression(now);

        // counter2 should not be affected
        assert_eq!(counter1.count(), 2);
        assert_eq!(counter2.count(), 1);
    }

    #[test]
    fn test_concurrent_clone_and_update() {
        use std::sync::Arc;
        use std::thread;

        let now = Instant::now();
        let counter = Arc::new(SuppressionCounter::new(now));

        let mut handles = vec![];

        // Thread 1: Updates counter
        let counter_clone1 = Arc::clone(&counter);
        handles.push(thread::spawn(move || {
            for _ in 0..100 {
                counter_clone1.record_suppression(now);
            }
        }));

        // Thread 2: Clones counter repeatedly
        let counter_clone2 = Arc::clone(&counter);
        handles.push(thread::spawn(move || {
            for _ in 0..100 {
                let _cloned = (*counter_clone2).clone();
            }
        }));

        for handle in handles {
            handle.join().unwrap();
        }

        // Should have at least some updates
        assert!(counter.count() > 1);
    }

    #[test]
    fn test_concurrent_reset_and_read() {
        use std::sync::Arc;
        use std::thread;

        let now = Instant::now();
        let counter = Arc::new(SuppressionCounter::new(now));

        let mut handles = vec![];

        // Thread 1: Repeatedly resets
        let counter_clone1 = Arc::clone(&counter);
        handles.push(thread::spawn(move || {
            for _ in 0..50 {
                counter_clone1.reset(now);
                thread::sleep(Duration::from_micros(10));
            }
        }));

        // Thread 2: Repeatedly records
        let counter_clone2 = Arc::clone(&counter);
        handles.push(thread::spawn(move || {
            for _ in 0..50 {
                counter_clone2.record_suppression(now);
                thread::sleep(Duration::from_micros(10));
            }
        }));

        // Thread 3: Repeatedly reads
        let counter_clone3 = Arc::clone(&counter);
        handles.push(thread::spawn(move || {
            for _ in 0..50 {
                let _count = counter_clone3.count();
                let _first = counter_clone3.first_suppressed();
                let _last = counter_clone3.last_suppressed();
                thread::sleep(Duration::from_micros(10));
            }
        }));

        for handle in handles {
            handle.join().unwrap();
        }

        // No assertions on final state due to race conditions,
        // but test should not panic or produce invalid data
    }

    #[test]
    fn test_very_large_suppression_count_stress() {
        let now = Instant::now();
        let counter = SuppressionCounter::new(now);

        // Simulate a huge number of suppressions
        for _ in 0..100_000 {
            counter.record_suppression(now);
        }

        assert_eq!(counter.count(), 100_001);
    }

    #[test]
    fn test_timestamp_persistence_over_time() {
        let start = Instant::now();
        let counter = SuppressionCounter::new(start);

        // Record suppressions over time
        for i in 1..=10 {
            let timestamp = start + Duration::from_millis(i * 100);
            counter.record_suppression(timestamp);
        }

        // First timestamp should be preserved
        let first = counter.first_suppressed();
        let duration_from_start = first.duration_since(start);
        assert!(duration_from_start < Duration::from_millis(10));

        // Last timestamp should be the most recent
        let last = counter.last_suppressed();
        let expected_last = start + Duration::from_millis(1000);
        let duration_diff = last.duration_since(expected_last);
        assert!(duration_diff < Duration::from_millis(10));
    }

    #[test]
    fn test_epoch_overflow_handling() {
        // Test with duration that would overflow u64 nanoseconds
        let base = SuppressionCounter::base_instant();
        let now = *base;

        let counter = SuppressionCounter::new(now);

        // Try to record at a time far in the future
        // Duration::from_secs(u64::MAX / 1_000_000_000) would be ~584 years
        // This tests saturation behavior
        let far_future = now + Duration::from_secs(600 * 365 * 24 * 3600); // 600 years

        counter.record_suppression(far_future);

        // Should not panic, timestamps should saturate gracefully
        let _first = counter.first_suppressed();
        let _last = counter.last_suppressed();
        assert!(counter.count() > 1);
    }

    #[test]
    fn test_base_instant_consistency() {
        // base_instant should be consistent across calls
        let base1 = SuppressionCounter::base_instant();
        let base2 = SuppressionCounter::base_instant();

        assert_eq!(base1, base2, "base_instant should be consistent");
    }

    #[test]
    fn test_nanos_conversion_roundtrip() {
        let now = Instant::now();
        let counter = SuppressionCounter::new(now);

        // Convert to nanos and back
        let first = counter.first_suppressed();
        let last = counter.last_suppressed();

        // Both should be close to 'now'
        assert!(first.duration_since(now) < Duration::from_millis(10));
        assert!(last.duration_since(now) < Duration::from_millis(10));
    }

    #[test]
    fn test_atomic_ordering_visibility() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        use std::thread;

        let now = Instant::now();
        let counter = Arc::new(SuppressionCounter::new(now));
        let done = Arc::new(AtomicBool::new(false));

        let counter_clone = Arc::clone(&counter);
        let done_clone = Arc::clone(&done);

        // Writer thread
        let writer = thread::spawn(move || {
            for i in 1..=100 {
                counter_clone.record_suppression(now + Duration::from_millis(i));
                thread::sleep(Duration::from_micros(10));
            }
            done_clone.store(true, Ordering::Release);
        });

        // Reader thread
        let counter_clone2 = Arc::clone(&counter);
        let done_clone2 = Arc::clone(&done);
        let reader = thread::spawn(move || {
            let mut last_count = 1;
            while !done_clone2.load(Ordering::Acquire) {
                let count = counter_clone2.count();
                // Count should never decrease
                assert!(count >= last_count, "Count should be monotonic");
                last_count = count;
                thread::sleep(Duration::from_micros(10));
            }
        });

        writer.join().unwrap();
        reader.join().unwrap();

        // Final count should be 101 (1 initial + 100 recorded)
        assert_eq!(counter.count(), 101);
    }

    #[test]
    fn test_summary_with_zero_duration() {
        let sig = EventSignature::simple("INFO", "Test");
        let now = Instant::now();
        let counter = SuppressionCounter::new(now);

        // Create summary immediately - same timestamp for first and last
        let summary = SuppressionSummary::from_counter(sig, &counter);

        assert_eq!(summary.count, 1);
        assert_eq!(summary.duration, Duration::from_secs(0));
        assert_eq!(summary.first_suppressed, summary.last_suppressed);
    }

    #[test]
    #[cfg(feature = "redis-storage")]
    fn test_snapshot_roundtrip() {
        let now = Instant::now();
        let counter = SuppressionCounter::new(now);

        counter.record_suppression(now + Duration::from_secs(1));
        counter.record_suppression(now + Duration::from_secs(2));

        let snapshot = counter.snapshot();

        let restored = SuppressionCounter::from_snapshot(
            snapshot.suppressed_count,
            snapshot.first_suppressed,
            snapshot.last_suppressed,
        );

        assert_eq!(counter.count(), restored.count());
        // Note: timestamps may have slight differences due to serialization precision
        let first_diff = counter
            .first_suppressed()
            .duration_since(restored.first_suppressed());
        let last_diff = counter
            .last_suppressed()
            .duration_since(restored.last_suppressed());

        assert!(first_diff < Duration::from_millis(1));
        assert!(last_diff < Duration::from_millis(1));
    }
}
