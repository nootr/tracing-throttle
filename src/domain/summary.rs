//! Suppression summaries and counters.
//!
//! This module tracks how many events have been suppressed and generates
//! periodic summaries for emission.

use crate::domain::{metadata::EventMetadata, signature::EventSignature};
use std::sync::{
    atomic::{AtomicU64, AtomicUsize, Ordering},
    Mutex, MutexGuard,
};
use std::time::{Duration, Instant};

/// Thread-safe counter for tracking suppressed events.
///
/// Uses atomics for cheap reads and a short critical section for updates that
/// must keep count and timestamp cursors consistent.
#[derive(Debug)]
pub struct SuppressionCounter {
    /// Guards multi-field updates to count and timestamp cursors
    state_lock: Mutex<()>,
    /// Total number of times this event was suppressed
    suppressed_count: AtomicUsize,
    /// Total number of suppressions already included in emitted summaries
    reported_count: AtomicUsize,
    /// Timestamp of first suppression (nanoseconds since epoch)
    first_suppressed_nanos: AtomicU64,
    /// Timestamp of last suppression (nanoseconds since epoch)
    last_suppressed_nanos: AtomicU64,
    /// Timestamp of the last suppression included in an emitted summary
    last_reported_nanos: AtomicU64,
    /// Timestamp of the first suppression not yet included in an emitted summary
    first_unreported_nanos: AtomicU64,
}

impl Clone for SuppressionCounter {
    fn clone(&self) -> Self {
        Self {
            state_lock: Mutex::new(()),
            suppressed_count: AtomicUsize::new(self.suppressed_count.load(Ordering::Relaxed)),
            reported_count: AtomicUsize::new(self.reported_count.load(Ordering::Relaxed)),
            first_suppressed_nanos: AtomicU64::new(
                self.first_suppressed_nanos.load(Ordering::Relaxed),
            ),
            last_suppressed_nanos: AtomicU64::new(
                self.last_suppressed_nanos.load(Ordering::Relaxed),
            ),
            last_reported_nanos: AtomicU64::new(self.last_reported_nanos.load(Ordering::Relaxed)),
            first_unreported_nanos: AtomicU64::new(
                self.first_unreported_nanos.load(Ordering::Relaxed),
            ),
        }
    }
}

impl SuppressionCounter {
    /// Create a new counter (initially zero suppressions).
    pub fn new(initial_timestamp: Instant) -> Self {
        let nanos = Self::instant_to_nanos(initial_timestamp);
        Self {
            state_lock: Mutex::new(()),
            suppressed_count: AtomicUsize::new(0),
            reported_count: AtomicUsize::new(0),
            first_suppressed_nanos: AtomicU64::new(nanos),
            last_suppressed_nanos: AtomicU64::new(nanos),
            last_reported_nanos: AtomicU64::new(nanos),
            first_unreported_nanos: AtomicU64::new(nanos),
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
        Self::from_snapshot_with_reported(
            suppressed_count,
            0,
            first_suppressed,
            last_suppressed,
            first_suppressed,
        )
    }

    /// Create a counter from a snapshot including reported suppressions.
    ///
    /// This is used by storage backends to persist active-emission progress.
    #[cfg(feature = "redis-storage")]
    pub fn from_snapshot_with_reported(
        suppressed_count: usize,
        reported_count: usize,
        first_suppressed: Instant,
        last_suppressed: Instant,
        last_reported: Instant,
    ) -> Self {
        let first_unreported = if reported_count == 0 {
            first_suppressed
        } else {
            last_reported
        };

        Self::from_snapshot_with_reported_and_first_unreported(
            suppressed_count,
            reported_count,
            first_suppressed,
            last_suppressed,
            last_reported,
            first_unreported,
        )
    }

    /// Create a counter from a snapshot including reported and unreported cursors.
    ///
    /// This is used by storage backends to preserve delta-summary timestamps.
    #[cfg(feature = "redis-storage")]
    pub fn from_snapshot_with_reported_and_first_unreported(
        suppressed_count: usize,
        reported_count: usize,
        first_suppressed: Instant,
        last_suppressed: Instant,
        last_reported: Instant,
        first_unreported: Instant,
    ) -> Self {
        let first_nanos = Self::instant_to_nanos(first_suppressed);
        let last_nanos = Self::instant_to_nanos(last_suppressed);
        let last_reported_nanos = Self::instant_to_nanos(last_reported);
        let first_unreported_nanos = Self::instant_to_nanos(first_unreported);
        let reported_count = reported_count.min(suppressed_count);

        Self {
            state_lock: Mutex::new(()),
            suppressed_count: AtomicUsize::new(suppressed_count),
            reported_count: AtomicUsize::new(reported_count),
            first_suppressed_nanos: AtomicU64::new(first_nanos),
            last_suppressed_nanos: AtomicU64::new(last_nanos),
            last_reported_nanos: AtomicU64::new(last_reported_nanos),
            first_unreported_nanos: AtomicU64::new(first_unreported_nanos),
        }
    }

    /// Record a new suppression event.
    pub fn record_suppression(&self, timestamp: Instant) {
        let _guard = self.lock_state();
        let nanos = Self::instant_to_nanos(timestamp);

        // Use AcqRel for fetch_add to synchronize with other threads
        let previous_count = self.suppressed_count.fetch_add(1, Ordering::AcqRel);
        let reported_count = self.reported_count.load(Ordering::Acquire);

        if previous_count == 0 {
            self.first_suppressed_nanos.store(nanos, Ordering::Release);
        }

        if previous_count == reported_count {
            self.first_unreported_nanos.store(nanos, Ordering::Release);
        }

        // Use Release to ensure timestamp update is visible
        self.last_suppressed_nanos.store(nanos, Ordering::Release);
    }

    /// Get the current suppression count.
    pub fn count(&self) -> usize {
        // Use Acquire to synchronize with Release/AcqRel operations
        self.suppressed_count.load(Ordering::Acquire)
    }

    /// Get the number of suppressions already included in emitted summaries.
    pub fn reported_count(&self) -> usize {
        self.reported_count.load(Ordering::Acquire)
    }

    /// Get the number of suppressions not yet included in emitted summaries.
    pub fn unreported_count(&self) -> usize {
        self.count().saturating_sub(self.reported_count())
    }

    /// Claim unreported suppressions for summary emission.
    ///
    /// Returns a snapshot of newly reported suppressions if at least `min_count`
    /// suppressions have accumulated since the last successful claim.
    pub fn claim_unreported(&self, min_count: usize) -> Option<ClaimedSuppressions> {
        let _guard = self.lock_state();
        let min_count = min_count.max(1);

        let total_count = self.count();
        let already_reported = self.reported_count();
        let unreported_count = total_count.saturating_sub(already_reported);

        if unreported_count < min_count {
            return None;
        }

        let first_nanos = self.first_unreported_nanos.load(Ordering::Acquire);
        let previous_last_reported_nanos = self.last_reported_nanos.load(Ordering::Acquire);
        let last_nanos = self.last_suppressed_nanos.load(Ordering::Acquire);

        self.reported_count.store(total_count, Ordering::Release);
        self.last_reported_nanos
            .store(last_nanos, Ordering::Release);
        self.first_unreported_nanos
            .store(last_nanos, Ordering::Release);

        let first_suppressed = Self::nanos_to_instant(first_nanos);
        let last_suppressed = Self::nanos_to_instant(last_nanos);

        Some(ClaimedSuppressions {
            previous_reported_count: already_reported,
            previous_last_reported: Self::nanos_to_instant(previous_last_reported_nanos),
            count: unreported_count,
            total_count,
            first_suppressed,
            last_suppressed,
            duration: last_suppressed.saturating_duration_since(first_suppressed),
        })
    }

    /// Roll back a previously claimed summary.
    ///
    /// If a later claim has already advanced the reported cursor, this rewinds to
    /// the older claim's starting point. That may cause a later summary to be
    /// emitted again, but prevents suppressions from being lost after a failed
    /// emission.
    pub fn rollback_claim(&self, claim: &ClaimedSuppressions) -> bool {
        let _guard = self.lock_state();
        let previous_last_reported_nanos = Self::instant_to_nanos(claim.previous_last_reported);
        let first_unreported_nanos = Self::instant_to_nanos(claim.first_suppressed);

        let current_reported = self.reported_count();

        if current_reported < claim.total_count {
            return current_reported == claim.previous_reported_count;
        }

        self.reported_count
            .store(claim.previous_reported_count, Ordering::Release);
        self.last_reported_nanos
            .store(previous_last_reported_nanos, Ordering::Release);
        self.first_unreported_nanos
            .store(first_unreported_nanos, Ordering::Release);
        true
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

    /// Get the timestamp of the last reported suppression.
    pub fn last_reported(&self) -> Instant {
        let nanos = self.last_reported_nanos.load(Ordering::Acquire);
        Self::nanos_to_instant(nanos)
    }

    /// Get the timestamp of the first unreported suppression.
    pub fn first_unreported(&self) -> Instant {
        let nanos = self.first_unreported_nanos.load(Ordering::Acquire);
        Self::nanos_to_instant(nanos)
    }

    /// Get a snapshot of the current state (for serialization).
    #[cfg(feature = "redis-storage")]
    pub fn snapshot(&self) -> super::summary::SuppressionSnapshot {
        super::summary::SuppressionSnapshot {
            suppressed_count: self.count(),
            reported_count: self.reported_count(),
            first_suppressed: self.first_suppressed(),
            last_suppressed: self.last_suppressed(),
            last_reported: self.last_reported(),
            first_unreported: self.first_unreported(),
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
        let _guard = self.lock_state();
        let nanos = Self::instant_to_nanos(timestamp);
        // Use Release for visibility
        self.suppressed_count.store(0, Ordering::Release);
        self.reported_count.store(0, Ordering::Release);
        self.first_suppressed_nanos.store(nanos, Ordering::Release);
        self.last_suppressed_nanos.store(nanos, Ordering::Release);
        self.last_reported_nanos.store(nanos, Ordering::Release);
        self.first_unreported_nanos.store(nanos, Ordering::Release);
    }

    /// Get the shared base instant for timestamp calculations.
    ///
    /// This ensures instant_to_nanos and nanos_to_instant use the same reference point.
    fn base_instant() -> &'static Instant {
        static BASE: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        BASE.get_or_init(Instant::now)
    }

    fn lock_state(&self) -> MutexGuard<'_, ()> {
        self.state_lock
            .lock()
            .unwrap_or_else(|err| err.into_inner())
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

/// Suppressions claimed for one summary emission.
#[derive(Debug, Clone)]
pub struct ClaimedSuppressions {
    /// Reported count before this claim
    pub previous_reported_count: usize,
    /// Last reported timestamp before this claim
    pub previous_last_reported: Instant,
    /// Number of newly reported suppressions
    pub count: usize,
    /// Total suppression count after this claim
    pub total_count: usize,
    /// Start of the claimed suppression period
    pub first_suppressed: Instant,
    /// End of the claimed suppression period
    pub last_suppressed: Instant,
    /// Duration covered by this claim
    pub duration: Duration,
}

/// A snapshot of suppression counter state (for serialization).
#[cfg(feature = "redis-storage")]
#[derive(Debug, Clone)]
pub struct SuppressionSnapshot {
    pub suppressed_count: usize,
    pub reported_count: usize,
    pub first_suppressed: Instant,
    pub last_suppressed: Instant,
    pub last_reported: Instant,
    pub first_unreported: Instant,
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

    /// Create a summary from claimed suppressions.
    pub fn from_claim(signature: EventSignature, claim: ClaimedSuppressions) -> Self {
        Self {
            signature,
            count: claim.count,
            first_suppressed: claim.first_suppressed,
            last_suppressed: claim.last_suppressed,
            duration: claim.duration,
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

    /// Create a summary from claimed suppressions with metadata.
    pub fn from_claim_with_metadata(
        signature: EventSignature,
        claim: ClaimedSuppressions,
        metadata: Option<EventMetadata>,
    ) -> Self {
        Self {
            signature,
            count: claim.count,
            first_suppressed: claim.first_suppressed,
            last_suppressed: claim.last_suppressed,
            duration: claim.duration,
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

        assert_eq!(counter.count(), 0);
        assert_eq!(counter.reported_count(), 0);
        assert_eq!(counter.unreported_count(), 0);

        counter.record_suppression(now);
        assert_eq!(counter.count(), 1);
        assert_eq!(counter.unreported_count(), 1);

        counter.record_suppression(now);
        assert_eq!(counter.count(), 2);
        assert_eq!(counter.unreported_count(), 2);
    }

    #[test]
    fn test_claim_unreported_suppressions() {
        let now = Instant::now();
        let counter = SuppressionCounter::new(now);

        counter.record_suppression(now);
        counter.record_suppression(now);

        let claim = counter.claim_unreported(1).expect("claim should exist");
        assert_eq!(claim.count, 2);
        assert_eq!(claim.total_count, 2);
        assert_eq!(counter.reported_count(), 2);
        assert_eq!(counter.unreported_count(), 0);

        assert!(
            counter.claim_unreported(1).is_none(),
            "already claimed suppressions should not be claimed again"
        );

        counter.record_suppression(now);
        let claim = counter.claim_unreported(1).expect("new claim should exist");
        assert_eq!(claim.count, 1);
        assert_eq!(claim.total_count, 3);
    }

    #[test]
    fn test_claim_unreported_uses_first_new_suppression_timestamp() {
        let now = Instant::now();
        let counter = SuppressionCounter::new(now);

        counter.record_suppression(now + Duration::from_secs(1));
        let first_claim = counter.claim_unreported(1).expect("claim should exist");
        assert_eq!(first_claim.first_suppressed, now + Duration::from_secs(1));
        assert_eq!(first_claim.last_suppressed, now + Duration::from_secs(1));

        counter.record_suppression(now + Duration::from_secs(60));
        let second_claim = counter.claim_unreported(1).expect("claim should exist");
        assert_eq!(second_claim.count, 1);
        assert_eq!(second_claim.first_suppressed, now + Duration::from_secs(60));
        assert_eq!(second_claim.last_suppressed, now + Duration::from_secs(60));
        assert_eq!(second_claim.duration, Duration::ZERO);
    }

    #[test]
    fn test_claim_unreported_respects_min_count() {
        let now = Instant::now();
        let counter = SuppressionCounter::new(now);

        for _ in 0..4 {
            counter.record_suppression(now);
        }

        assert!(counter.claim_unreported(5).is_none());
        assert_eq!(counter.reported_count(), 0);
        assert_eq!(counter.unreported_count(), 4);

        counter.record_suppression(now);
        let claim = counter.claim_unreported(5).expect("claim should exist");
        assert_eq!(claim.count, 5);
        assert_eq!(counter.reported_count(), 5);
    }

    #[test]
    fn test_claim_unreported_min_count_zero_requires_suppressions() {
        let now = Instant::now();
        let counter = SuppressionCounter::new(now);

        assert!(
            counter.claim_unreported(0).is_none(),
            "zero min_count should not create an empty claim"
        );

        counter.record_suppression(now);
        let claim = counter.claim_unreported(0).expect("claim should exist");
        assert_eq!(claim.count, 1);

        assert!(
            counter.claim_unreported(0).is_none(),
            "reported suppressions should not create a zero-count claim"
        );
    }

    #[test]
    fn test_rollback_claim_restores_unreported_suppressions() {
        let now = Instant::now();
        let counter = SuppressionCounter::new(now);

        counter.record_suppression(now);
        counter.record_suppression(now);

        let claim = counter.claim_unreported(1).expect("claim should exist");
        assert_eq!(counter.reported_count(), 2);
        assert_eq!(counter.unreported_count(), 0);

        assert!(counter.rollback_claim(&claim));
        assert_eq!(counter.reported_count(), 0);
        assert_eq!(counter.unreported_count(), 2);

        let claim = counter
            .claim_unreported(1)
            .expect("rolled back suppressions should be claimable again");
        assert_eq!(claim.count, 2);
    }

    #[test]
    fn test_rollback_superseded_claim_restores_full_unreported_range() {
        let now = Instant::now();
        let counter = SuppressionCounter::new(now);

        counter.record_suppression(now + Duration::from_secs(1));
        counter.record_suppression(now + Duration::from_secs(2));
        let first_claim = counter.claim_unreported(1).expect("claim should exist");

        counter.record_suppression(now + Duration::from_secs(3));
        let second_claim = counter
            .claim_unreported(1)
            .expect("later claim should exist");
        assert_eq!(second_claim.count, 1);
        assert_eq!(counter.reported_count(), 3);

        assert!(counter.rollback_claim(&first_claim));
        assert_eq!(counter.reported_count(), 0);
        assert_eq!(counter.unreported_count(), 3);

        let retry_claim = counter
            .claim_unreported(1)
            .expect("rolled back suppressions should retry");
        assert_eq!(retry_claim.count, 3);
        assert_eq!(retry_claim.first_suppressed, now + Duration::from_secs(1));
        assert_eq!(retry_claim.last_suppressed, now + Duration::from_secs(3));
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

        // First should be approximately the first recorded suppression
        assert!(first.saturating_duration_since(later) < Duration::from_millis(5));

        // Last should be approximately later
        assert!(last.saturating_duration_since(later) < Duration::from_millis(5));
    }

    #[test]
    fn test_suppression_counter_reset() {
        let now = Instant::now();
        let counter = SuppressionCounter::new(now);

        counter.record_suppression(now);
        counter.record_suppression(now);
        assert_eq!(counter.count(), 2);

        counter.reset(now);
        assert_eq!(counter.count(), 0);
        assert_eq!(counter.reported_count(), 0);
        assert_eq!(counter.unreported_count(), 0);
    }

    #[test]
    fn test_suppression_summary_creation() {
        let sig = EventSignature::simple("INFO", "Test message");
        let start = Instant::now();
        let counter = SuppressionCounter::new(start);

        let first = Instant::now();
        counter.record_suppression(first);
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

        assert!(message.contains("suppressed 1 times"));
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

        assert_eq!(counter.count(), 10_000);
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

        // 10 threads * 100 updates = 1000
        assert_eq!(counter.count(), 1000);
    }

    #[test]
    fn test_zero_duration_summary() {
        let sig = EventSignature::simple("INFO", "Test");
        let now = Instant::now();
        let counter = SuppressionCounter::new(now);

        // Immediately create summary (same timestamp)
        let summary = SuppressionSummary::from_counter(sig, &counter);

        assert_eq!(summary.count, 0);
        assert!(summary.duration < Duration::from_millis(1));
    }

    #[test]
    fn test_reset_multiple_times() {
        let now = Instant::now();
        let counter = SuppressionCounter::new(now);

        counter.record_suppression(now);
        assert_eq!(counter.count(), 1);

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
        assert_eq!(counter.reported_count(), cloned.reported_count());
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
        let _claim = counter1.claim_unreported(1);

        // counter2 should not be affected
        assert_eq!(counter1.count(), 1);
        assert_eq!(counter1.reported_count(), 1);
        assert_eq!(counter2.count(), 0);
        assert_eq!(counter2.reported_count(), 0);
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

        assert_eq!(counter.count(), 100_000);
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

        // First recorded suppression timestamp should be preserved
        let first = counter.first_suppressed();
        let expected_first = start + Duration::from_millis(100);
        let duration_from_first = first.duration_since(expected_first);
        assert!(duration_from_first < Duration::from_millis(10));

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
        assert!(counter.count() > 0);
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
            let mut last_count = 0;
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

        // Final count should be 100
        assert_eq!(counter.count(), 100);
    }

    #[test]
    fn test_summary_with_zero_duration() {
        let sig = EventSignature::simple("INFO", "Test");
        let now = Instant::now();
        let counter = SuppressionCounter::new(now);

        // Create summary immediately - same timestamp for first and last
        let summary = SuppressionSummary::from_counter(sig, &counter);

        assert_eq!(summary.count, 0);
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
        let _claim = counter.claim_unreported(1);
        counter.record_suppression(now + Duration::from_secs(3));

        let snapshot = counter.snapshot();

        let restored = SuppressionCounter::from_snapshot(
            snapshot.suppressed_count,
            snapshot.first_suppressed,
            snapshot.last_suppressed,
        );

        assert_eq!(counter.count(), restored.count());
        assert_eq!(0, restored.reported_count());

        let restored_with_reported = SuppressionCounter::from_snapshot_with_reported(
            snapshot.suppressed_count,
            snapshot.reported_count,
            snapshot.first_suppressed,
            snapshot.last_suppressed,
            snapshot.last_reported,
        );
        assert_eq!(counter.count(), restored_with_reported.count());
        assert_eq!(
            counter.reported_count(),
            restored_with_reported.reported_count()
        );

        let restored_with_unreported =
            SuppressionCounter::from_snapshot_with_reported_and_first_unreported(
                snapshot.suppressed_count,
                snapshot.reported_count,
                snapshot.first_suppressed,
                snapshot.last_suppressed,
                snapshot.last_reported,
                snapshot.first_unreported,
            );
        assert_eq!(counter.count(), restored_with_unreported.count());
        assert_eq!(
            counter.reported_count(),
            restored_with_unreported.reported_count()
        );

        // Note: timestamps may have slight differences due to serialization precision
        let first_diff = counter
            .first_suppressed()
            .duration_since(restored.first_suppressed());
        let last_diff = counter
            .last_suppressed()
            .duration_since(restored.last_suppressed());
        let last_reported_diff = counter
            .last_reported()
            .duration_since(restored_with_reported.last_reported());
        let first_unreported_diff = counter
            .first_unreported()
            .duration_since(restored_with_unreported.first_unreported());

        assert!(first_diff < Duration::from_millis(1));
        assert!(last_diff < Duration::from_millis(1));
        assert!(last_reported_diff < Duration::from_millis(1));
        assert!(first_unreported_diff < Duration::from_millis(1));
    }
}
