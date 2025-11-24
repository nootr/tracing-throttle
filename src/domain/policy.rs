//! Rate limiting policies for event suppression.
//!
//! This module defines the core trait for rate limiting policies and provides
//! several built-in implementations.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Decision made by a rate limiting policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyDecision {
    /// Allow the event to be emitted
    Allow,
    /// Suppress the event (don't emit it)
    Suppress,
}

/// Trait for implementing rate limiting policies.
///
/// Policies determine whether an event should be allowed or suppressed based
/// on historical event patterns.
pub trait RateLimitPolicy: Send + Sync {
    /// Register a new event occurrence and decide whether to allow or suppress it.
    ///
    /// # Arguments
    /// * `timestamp` - When the event occurred
    ///
    /// # Returns
    /// A `PolicyDecision` indicating whether to allow or suppress the event.
    fn register_event(&mut self, timestamp: Instant) -> PolicyDecision;

    /// Reset the policy state.
    ///
    /// Called when starting a new tracking period or when clearing history.
    fn reset(&mut self);
}

/// Count-based rate limiting policy.
///
/// Allows up to N events, then suppresses all subsequent events.
///
/// # Example
/// ```
/// use tracing_throttle::{CountBasedPolicy, RateLimitPolicy};
/// use std::time::Instant;
///
/// let mut policy = CountBasedPolicy::new(3);
/// let now = Instant::now();
///
/// // First 3 events allowed
/// assert!(policy.register_event(now).is_allow());
/// assert!(policy.register_event(now).is_allow());
/// assert!(policy.register_event(now).is_allow());
///
/// // 4th and beyond suppressed
/// assert!(policy.register_event(now).is_suppress());
/// assert!(policy.register_event(now).is_suppress());
/// ```
#[derive(Debug, Clone)]
pub struct CountBasedPolicy {
    max_count: usize,
    current_count: usize,
}

impl CountBasedPolicy {
    /// Create a new count-based policy.
    ///
    /// # Arguments
    /// * `max_count` - Maximum number of events to allow before suppressing
    pub fn new(max_count: usize) -> Self {
        Self {
            max_count,
            current_count: 0,
        }
    }
}

impl RateLimitPolicy for CountBasedPolicy {
    fn register_event(&mut self, _timestamp: Instant) -> PolicyDecision {
        self.current_count += 1;
        if self.current_count <= self.max_count {
            PolicyDecision::Allow
        } else {
            PolicyDecision::Suppress
        }
    }

    fn reset(&mut self) {
        self.current_count = 0;
    }
}

/// Time-window rate limiting policy.
///
/// Allows up to K events within a sliding time window. Events outside the
/// window are automatically expired.
///
/// # Example
/// ```
/// use tracing_throttle::{TimeWindowPolicy, RateLimitPolicy};
/// use std::time::{Duration, Instant};
///
/// let mut policy = TimeWindowPolicy::new(2, Duration::from_secs(60));
/// let now = Instant::now();
///
/// // First 2 events allowed
/// assert!(policy.register_event(now).is_allow());
/// assert!(policy.register_event(now).is_allow());
///
/// // 3rd event suppressed (within window)
/// assert!(policy.register_event(now).is_suppress());
///
/// // After window expires, events are allowed again
/// let after_window = now + Duration::from_secs(61);
/// assert!(policy.register_event(after_window).is_allow());
/// assert!(policy.register_event(after_window).is_allow());
/// ```
#[derive(Debug, Clone)]
pub struct TimeWindowPolicy {
    max_events: usize,
    window_duration: Duration,
    event_timestamps: VecDeque<Instant>,
}

impl TimeWindowPolicy {
    /// Create a new time-window policy.
    ///
    /// # Arguments
    /// * `max_events` - Maximum events allowed in the window
    /// * `window_duration` - Length of the sliding time window
    pub fn new(max_events: usize, window_duration: Duration) -> Self {
        Self {
            max_events,
            window_duration,
            event_timestamps: VecDeque::new(),
        }
    }

    /// Remove expired events from the window.
    fn expire_old_events(&mut self, current_time: Instant) {
        while let Some(&oldest) = self.event_timestamps.front() {
            if current_time.saturating_duration_since(oldest) > self.window_duration {
                self.event_timestamps.pop_front();
            } else {
                break;
            }
        }
    }
}

impl RateLimitPolicy for TimeWindowPolicy {
    fn register_event(&mut self, timestamp: Instant) -> PolicyDecision {
        self.expire_old_events(timestamp);

        if self.event_timestamps.len() < self.max_events {
            self.event_timestamps.push_back(timestamp);
            PolicyDecision::Allow
        } else {
            PolicyDecision::Suppress
        }
    }

    fn reset(&mut self) {
        self.event_timestamps.clear();
    }
}

/// Exponential backoff policy.
///
/// Allows events at exponentially increasing intervals: 1st, 2nd, 4th, 8th, 16th, etc.
/// Useful for extremely noisy logs.
///
/// # Example
/// ```
/// use tracing_throttle::{ExponentialBackoffPolicy, RateLimitPolicy};
/// use std::time::Instant;
///
/// let mut policy = ExponentialBackoffPolicy::new();
/// let now = Instant::now();
///
/// assert!(policy.register_event(now).is_allow());  // 1st
/// assert!(policy.register_event(now).is_allow());  // 2nd
/// assert!(policy.register_event(now).is_suppress()); // 3rd - suppressed
/// assert!(policy.register_event(now).is_allow());  // 4th
/// ```
#[derive(Debug, Clone)]
pub struct ExponentialBackoffPolicy {
    event_count: u64,
    next_allowed: u64,
}

impl ExponentialBackoffPolicy {
    /// Create a new exponential backoff policy.
    pub fn new() -> Self {
        Self {
            event_count: 0,
            next_allowed: 1,
        }
    }
}

impl Default for ExponentialBackoffPolicy {
    fn default() -> Self {
        Self::new()
    }
}

impl RateLimitPolicy for ExponentialBackoffPolicy {
    fn register_event(&mut self, _timestamp: Instant) -> PolicyDecision {
        self.event_count += 1;

        if self.event_count == self.next_allowed {
            self.next_allowed = self.next_allowed.saturating_mul(2);
            PolicyDecision::Allow
        } else {
            PolicyDecision::Suppress
        }
    }

    fn reset(&mut self) {
        self.event_count = 0;
        self.next_allowed = 1;
    }
}

/// Convenience enum for common policy types.
#[derive(Debug, Clone)]
pub enum Policy {
    /// Count-based policy
    CountBased(CountBasedPolicy),
    /// Time-window policy
    TimeWindow(TimeWindowPolicy),
    /// Exponential backoff policy
    ExponentialBackoff(ExponentialBackoffPolicy),
}

impl Policy {
    /// Create a count-based policy.
    pub fn count_based(max_count: usize) -> Self {
        Policy::CountBased(CountBasedPolicy::new(max_count))
    }

    /// Create a time-window policy.
    pub fn time_window(max_events: usize, window: Duration) -> Self {
        Policy::TimeWindow(TimeWindowPolicy::new(max_events, window))
    }

    /// Create an exponential backoff policy.
    pub fn exponential_backoff() -> Self {
        Policy::ExponentialBackoff(ExponentialBackoffPolicy::new())
    }
}

impl RateLimitPolicy for Policy {
    fn register_event(&mut self, timestamp: Instant) -> PolicyDecision {
        match self {
            Policy::CountBased(p) => p.register_event(timestamp),
            Policy::TimeWindow(p) => p.register_event(timestamp),
            Policy::ExponentialBackoff(p) => p.register_event(timestamp),
        }
    }

    fn reset(&mut self) {
        match self {
            Policy::CountBased(p) => p.reset(),
            Policy::TimeWindow(p) => p.reset(),
            Policy::ExponentialBackoff(p) => p.reset(),
        }
    }
}

impl PolicyDecision {
    /// Check if this decision is Allow.
    pub fn is_allow(&self) -> bool {
        matches!(self, PolicyDecision::Allow)
    }

    /// Check if this decision is Suppress.
    pub fn is_suppress(&self) -> bool {
        matches!(self, PolicyDecision::Suppress)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_count_based_policy() {
        let mut policy = CountBasedPolicy::new(3);
        let now = Instant::now();

        assert_eq!(policy.register_event(now), PolicyDecision::Allow);
        assert_eq!(policy.register_event(now), PolicyDecision::Allow);
        assert_eq!(policy.register_event(now), PolicyDecision::Allow);
        assert_eq!(policy.register_event(now), PolicyDecision::Suppress);
        assert_eq!(policy.register_event(now), PolicyDecision::Suppress);

        policy.reset();
        assert_eq!(policy.register_event(now), PolicyDecision::Allow);
    }

    #[test]
    fn test_time_window_policy() {
        let mut policy = TimeWindowPolicy::new(2, Duration::from_secs(1));
        let now = Instant::now();

        assert_eq!(policy.register_event(now), PolicyDecision::Allow);
        assert_eq!(policy.register_event(now), PolicyDecision::Allow);
        assert_eq!(policy.register_event(now), PolicyDecision::Suppress);

        // After window expires, should allow again
        let later = now + Duration::from_secs(2);
        assert_eq!(policy.register_event(later), PolicyDecision::Allow);
    }

    #[test]
    fn test_exponential_backoff_policy() {
        let mut policy = ExponentialBackoffPolicy::new();
        let now = Instant::now();

        // 1st allowed
        assert_eq!(policy.register_event(now), PolicyDecision::Allow);
        // 2nd allowed
        assert_eq!(policy.register_event(now), PolicyDecision::Allow);
        // 3rd suppressed
        assert_eq!(policy.register_event(now), PolicyDecision::Suppress);
        // 4th allowed
        assert_eq!(policy.register_event(now), PolicyDecision::Allow);
        // 5th, 6th, 7th suppressed
        assert_eq!(policy.register_event(now), PolicyDecision::Suppress);
        assert_eq!(policy.register_event(now), PolicyDecision::Suppress);
        assert_eq!(policy.register_event(now), PolicyDecision::Suppress);
        // 8th allowed
        assert_eq!(policy.register_event(now), PolicyDecision::Allow);
    }

    #[test]
    fn test_policy_enum() {
        let mut policy = Policy::count_based(2);
        let now = Instant::now();

        assert!(policy.register_event(now).is_allow());
        assert!(policy.register_event(now).is_allow());
        assert!(policy.register_event(now).is_suppress());
    }

    // Edge case tests
    #[test]
    fn test_count_based_policy_zero_limit() {
        let mut policy = CountBasedPolicy::new(0);
        let now = Instant::now();

        // All events should be suppressed
        assert_eq!(policy.register_event(now), PolicyDecision::Suppress);
        assert_eq!(policy.register_event(now), PolicyDecision::Suppress);
    }

    #[test]
    fn test_count_based_policy_one_limit() {
        let mut policy = CountBasedPolicy::new(1);
        let now = Instant::now();

        // Only first event allowed
        assert_eq!(policy.register_event(now), PolicyDecision::Allow);
        assert_eq!(policy.register_event(now), PolicyDecision::Suppress);
        assert_eq!(policy.register_event(now), PolicyDecision::Suppress);
    }

    #[test]
    fn test_count_based_policy_reset() {
        let mut policy = CountBasedPolicy::new(2);
        let now = Instant::now();

        // Use up the limit
        assert_eq!(policy.register_event(now), PolicyDecision::Allow);
        assert_eq!(policy.register_event(now), PolicyDecision::Allow);
        assert_eq!(policy.register_event(now), PolicyDecision::Suppress);

        // Reset should restore the limit
        policy.reset();
        assert_eq!(policy.register_event(now), PolicyDecision::Allow);
        assert_eq!(policy.register_event(now), PolicyDecision::Allow);
        assert_eq!(policy.register_event(now), PolicyDecision::Suppress);
    }

    #[test]
    fn test_time_window_policy_zero_duration() {
        let mut policy = TimeWindowPolicy::new(2, Duration::from_secs(0));
        let now = Instant::now();

        // With zero duration, window immediately expires
        assert_eq!(policy.register_event(now), PolicyDecision::Allow);
        assert_eq!(policy.register_event(now), PolicyDecision::Allow);
    }

    #[test]
    fn test_time_window_policy_rapid_events() {
        let mut policy = TimeWindowPolicy::new(3, Duration::from_millis(100));
        let now = Instant::now();

        // Rapid fire events
        for i in 0..10 {
            let decision = policy.register_event(now);
            if i < 3 {
                assert_eq!(
                    decision,
                    PolicyDecision::Allow,
                    "Event {} should be allowed",
                    i
                );
            } else {
                assert_eq!(
                    decision,
                    PolicyDecision::Suppress,
                    "Event {} should be suppressed",
                    i
                );
            }
        }
    }

    #[test]
    fn test_time_window_policy_reset() {
        let mut policy = TimeWindowPolicy::new(2, Duration::from_secs(60));
        let now = Instant::now();

        // Use up limit
        assert_eq!(policy.register_event(now), PolicyDecision::Allow);
        assert_eq!(policy.register_event(now), PolicyDecision::Allow);
        assert_eq!(policy.register_event(now), PolicyDecision::Suppress);

        // Reset should clear the window
        policy.reset();
        assert_eq!(policy.register_event(now), PolicyDecision::Allow);
    }

    #[test]
    fn test_exponential_backoff_large_count() {
        let mut policy = ExponentialBackoffPolicy::new();
        let now = Instant::now();

        let expected_allowed = [0, 1, 3, 7, 15, 31, 63]; // 0-indexed: 1st, 2nd, 4th, 8th, 16th, 32nd, 64th

        for i in 0..100 {
            let decision = policy.register_event(now);
            if expected_allowed.contains(&i) {
                assert_eq!(
                    decision,
                    PolicyDecision::Allow,
                    "Event {} should be allowed",
                    i + 1
                );
            } else {
                assert_eq!(
                    decision,
                    PolicyDecision::Suppress,
                    "Event {} should be suppressed",
                    i + 1
                );
            }
        }
    }

    #[test]
    fn test_exponential_backoff_reset() {
        let mut policy = ExponentialBackoffPolicy::new();
        let now = Instant::now();

        // Progress through first few events
        assert_eq!(policy.register_event(now), PolicyDecision::Allow); // 1st
        assert_eq!(policy.register_event(now), PolicyDecision::Allow); // 2nd
        assert_eq!(policy.register_event(now), PolicyDecision::Suppress); // 3rd

        // Reset should start over
        policy.reset();
        assert_eq!(policy.register_event(now), PolicyDecision::Allow); // 1st again
    }
}
