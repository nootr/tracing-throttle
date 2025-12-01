//! Rate limiter coordination logic.
//!
//! The rate limiter decides whether events should be allowed or suppressed
//! based on policies and tracks suppression counts.

use crate::application::circuit_breaker::CircuitBreaker;
use crate::application::metrics::Metrics;
use crate::application::ports::Storage;
use crate::application::registry::SuppressionRegistry;
use crate::domain::{
    metadata::EventMetadata,
    policy::{PolicyDecision, RateLimitPolicy},
    signature::EventSignature,
};
use std::panic;
use std::sync::Arc;

/// Decision about how to handle an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitDecision {
    /// Allow the event to pass through
    Allow,
    /// Suppress the event
    Suppress,
}

/// Coordinates rate limiting decisions.
#[derive(Clone)]
pub struct RateLimiter<S>
where
    S: Storage<EventSignature, crate::application::registry::EventState> + Clone,
{
    registry: SuppressionRegistry<S>,
    metrics: Metrics,
    circuit_breaker: Arc<CircuitBreaker>,
}

impl<S> RateLimiter<S>
where
    S: Storage<EventSignature, crate::application::registry::EventState> + Clone,
{
    /// Create a new rate limiter.
    ///
    /// # Arguments
    /// * `registry` - The suppression registry (which contains the clock)
    /// * `metrics` - Metrics tracker
    /// * `circuit_breaker` - Circuit breaker for fail-safe operation
    pub fn new(
        registry: SuppressionRegistry<S>,
        metrics: Metrics,
        circuit_breaker: Arc<CircuitBreaker>,
    ) -> Self {
        Self {
            registry,
            metrics,
            circuit_breaker,
        }
    }

    /// Process an event and decide whether to allow or suppress it.
    ///
    /// # Arguments
    /// * `signature` - The event signature
    ///
    /// # Returns
    /// A `LimitDecision` indicating whether to allow or suppress the event.
    ///
    /// # Fail-Safe Behavior
    /// If rate limiting operations fail (circuit breaker open), this method fails open
    /// and allows all events through to preserve observability.
    ///
    /// # Performance
    /// This method is designed for the hot path:
    /// - Fast hash lookup in sharded map
    /// - Lock-free atomic operations where possible
    /// - No allocations in common case
    pub fn check_event(&self, signature: EventSignature) -> LimitDecision {
        // Check circuit breaker state
        if !self.circuit_breaker.allow_request() {
            // Circuit is open, fail open (allow all events)
            self.metrics.record_allowed();
            return LimitDecision::Allow;
        }

        // Attempt rate limiting operation with panic protection
        let result = panic::catch_unwind(panic::AssertUnwindSafe(|| {
            self.registry.with_event_state(signature, |state, now| {
                // Ask the policy whether to allow this event
                let decision = state.policy.register_event(now);

                match decision {
                    PolicyDecision::Allow => LimitDecision::Allow,
                    PolicyDecision::Suppress => {
                        // Record the suppression
                        state.counter.record_suppression(now);
                        LimitDecision::Suppress
                    }
                }
            })
        }));

        let decision = match result {
            Ok(decision) => {
                // Operation succeeded
                self.circuit_breaker.record_success();
                decision
            }
            Err(_) => {
                // Operation panicked, record failure and fail open
                self.circuit_breaker.record_failure();
                LimitDecision::Allow
            }
        };

        // Record metrics
        match decision {
            LimitDecision::Allow => self.metrics.record_allowed(),
            LimitDecision::Suppress => self.metrics.record_suppressed(),
        }

        decision
    }

    /// Process an event with metadata and decide whether to allow or suppress it.
    ///
    /// This method captures event metadata on first occurrence for human-readable summaries.
    ///
    /// # Arguments
    /// * `signature` - The event signature
    /// * `metadata` - Event details (level, message, target, fields)
    ///
    /// # Returns
    /// A `LimitDecision` indicating whether to allow or suppress the event.
    ///
    /// # Fail-Safe Behavior
    /// Same as `check_event`: fails open if rate limiting operations fail.
    pub fn check_event_with_metadata(
        &self,
        signature: EventSignature,
        metadata: EventMetadata,
    ) -> LimitDecision {
        // Check circuit breaker state
        if !self.circuit_breaker.allow_request() {
            // Circuit is open, fail open (allow all events)
            self.metrics.record_allowed();
            return LimitDecision::Allow;
        }

        // Attempt rate limiting operation with panic protection
        let result = panic::catch_unwind(panic::AssertUnwindSafe(|| {
            self.registry.with_event_state(signature, |state, now| {
                // Capture metadata on first occurrence
                state.set_metadata(metadata);

                // Ask the policy whether to allow this event
                let decision = state.policy.register_event(now);

                match decision {
                    PolicyDecision::Allow => LimitDecision::Allow,
                    PolicyDecision::Suppress => {
                        // Record the suppression
                        state.counter.record_suppression(now);
                        LimitDecision::Suppress
                    }
                }
            })
        }));

        let decision = match result {
            Ok(decision) => {
                // Operation succeeded
                self.circuit_breaker.record_success();
                decision
            }
            Err(_) => {
                // Operation panicked, record failure and fail open
                self.circuit_breaker.record_failure();
                LimitDecision::Allow
            }
        };

        // Record metrics
        match decision {
            LimitDecision::Allow => self.metrics.record_allowed(),
            LimitDecision::Suppress => self.metrics.record_suppressed(),
        }

        decision
    }

    /// Get a reference to the registry.
    pub fn registry(&self) -> &SuppressionRegistry<S> {
        &self.registry
    }

    /// Get a reference to the metrics.
    pub fn metrics(&self) -> &Metrics {
        &self.metrics
    }

    /// Get a reference to the circuit breaker.
    pub fn circuit_breaker(&self) -> &Arc<CircuitBreaker> {
        &self.circuit_breaker
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::policy::Policy;
    use crate::infrastructure::clock::SystemClock;
    use crate::infrastructure::mocks::MockClock;
    use crate::infrastructure::storage::ShardedStorage;
    use std::sync::Arc;
    use std::time::Instant;

    #[test]
    fn test_rate_limiter_basic() {
        let storage = Arc::new(ShardedStorage::new());
        let clock = Arc::new(SystemClock::new());
        let policy = Policy::count_based(2).unwrap();
        let registry = SuppressionRegistry::new(storage, clock, policy);
        let limiter = RateLimiter::new(registry, Metrics::new(), Arc::new(CircuitBreaker::new()));

        let sig = EventSignature::simple("INFO", "Test message");

        // First two events allowed
        assert_eq!(limiter.check_event(sig), LimitDecision::Allow);
        assert_eq!(limiter.check_event(sig), LimitDecision::Allow);

        // Third and beyond suppressed
        assert_eq!(limiter.check_event(sig), LimitDecision::Suppress);
        assert_eq!(limiter.check_event(sig), LimitDecision::Suppress);
    }

    #[test]
    fn test_rate_limiter_with_mock_clock() {
        use std::time::Duration;

        let storage = Arc::new(ShardedStorage::new());
        let mock_clock = Arc::new(MockClock::new(Instant::now()));
        let policy = Policy::time_window(2, Duration::from_secs(60)).unwrap();
        let registry = SuppressionRegistry::new(storage, mock_clock.clone(), policy);
        let limiter = RateLimiter::new(registry, Metrics::new(), Arc::new(CircuitBreaker::new()));

        let sig = EventSignature::simple("INFO", "Test");

        // First 2 allowed
        assert_eq!(limiter.check_event(sig), LimitDecision::Allow);
        assert_eq!(limiter.check_event(sig), LimitDecision::Allow);

        // 3rd suppressed
        assert_eq!(limiter.check_event(sig), LimitDecision::Suppress);

        // Advance time by 61 seconds
        mock_clock.advance(Duration::from_secs(61));

        // Should allow again
        assert_eq!(limiter.check_event(sig), LimitDecision::Allow);
    }

    #[test]
    fn test_rate_limiter_different_signatures() {
        let storage = Arc::new(ShardedStorage::new());
        let clock = Arc::new(SystemClock::new());
        let policy = Policy::count_based(1).unwrap();
        let registry = SuppressionRegistry::new(storage, clock, policy);
        let limiter = RateLimiter::new(registry, Metrics::new(), Arc::new(CircuitBreaker::new()));

        let sig1 = EventSignature::simple("INFO", "Message 1");
        let sig2 = EventSignature::simple("INFO", "Message 2");

        // Each signature has independent limits
        assert_eq!(limiter.check_event(sig1), LimitDecision::Allow);
        assert_eq!(limiter.check_event(sig2), LimitDecision::Allow);

        assert_eq!(limiter.check_event(sig1), LimitDecision::Suppress);
        assert_eq!(limiter.check_event(sig2), LimitDecision::Suppress);
    }

    #[test]
    fn test_rate_limiter_suppression_counting() {
        let storage = Arc::new(ShardedStorage::new());
        let clock = Arc::new(SystemClock::new());
        let policy = Policy::count_based(1).unwrap();
        let registry = SuppressionRegistry::new(storage, clock, policy);
        let limiter = RateLimiter::new(
            registry.clone(),
            Metrics::new(),
            Arc::new(CircuitBreaker::new()),
        );

        let sig = EventSignature::simple("INFO", "Test");

        // Allow first
        assert_eq!(limiter.check_event(sig), LimitDecision::Allow);

        // Suppress next 3
        assert_eq!(limiter.check_event(sig), LimitDecision::Suppress);
        assert_eq!(limiter.check_event(sig), LimitDecision::Suppress);
        assert_eq!(limiter.check_event(sig), LimitDecision::Suppress);

        // Check counter - initial creation counts as 1, plus 3 recorded = 4 total
        registry.with_event_state(sig, |state, _now| {
            assert_eq!(state.counter.count(), 4);
        });
    }

    #[test]
    fn test_concurrent_rate_limiting() {
        use std::thread;

        let storage = Arc::new(ShardedStorage::new());
        let clock = Arc::new(SystemClock::new());
        let policy = Policy::count_based(50).unwrap();
        let registry = SuppressionRegistry::new(storage, clock, policy);
        let limiter = Arc::new(RateLimiter::new(
            registry,
            Metrics::new(),
            Arc::new(CircuitBreaker::new()),
        ));

        let sig = EventSignature::simple("INFO", "Concurrent test");
        let mut handles = vec![];

        for _ in 0..10 {
            let limiter_clone = Arc::clone(&limiter);
            let handle = thread::spawn(move || {
                let mut allowed = 0;
                let mut suppressed = 0;

                for _ in 0..20 {
                    match limiter_clone.check_event(sig) {
                        LimitDecision::Allow => allowed += 1,
                        LimitDecision::Suppress => suppressed += 1,
                    }
                }

                (allowed, suppressed)
            });
            handles.push(handle);
        }

        let mut total_allowed = 0;
        let mut total_suppressed = 0;

        for handle in handles {
            let (allowed, suppressed) = handle.join().unwrap();
            total_allowed += allowed;
            total_suppressed += suppressed;
        }

        // Total events = 10 threads * 20 events = 200
        assert_eq!(total_allowed + total_suppressed, 200);

        // Should have allowed at most 50 (policy limit)
        assert!(total_allowed <= 50);

        // Should have suppressed the rest
        assert!(total_suppressed >= 150);
    }
}
