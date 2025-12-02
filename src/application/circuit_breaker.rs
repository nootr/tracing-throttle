//! Circuit breaker for fail-safe rate limiting.
//!
//! Implements a circuit breaker pattern to prevent cascading failures when
//! rate limiting operations fail. Uses a fail-open strategy: when the circuit
//! opens due to failures, all events are allowed through to preserve observability.

use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Circuit breaker states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    /// Circuit is closed, operating normally
    Closed = 0,
    /// Circuit is open due to failures, failing open (allowing all events)
    Open = 1,
    /// Circuit is testing if system has recovered
    HalfOpen = 2,
}

impl From<u8> for CircuitState {
    fn from(value: u8) -> Self {
        match value {
            0 => CircuitState::Closed,
            1 => CircuitState::Open,
            2 => CircuitState::HalfOpen,
            _ => CircuitState::Closed, // Default to closed for invalid values
        }
    }
}

/// Configuration for circuit breaker behavior.
#[derive(Debug, Clone)]
pub struct CircuitBreakerConfig {
    /// Number of consecutive failures before opening circuit
    pub failure_threshold: u32,
    /// Duration to wait before attempting recovery
    pub recovery_timeout: Duration,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 5,
            recovery_timeout: Duration::from_secs(30),
        }
    }
}

/// Circuit breaker for protecting rate limiting operations.
///
/// When failures occur, the circuit breaker opens and fails open,
/// allowing all events through to preserve observability.
#[derive(Debug)]
pub struct CircuitBreaker {
    state: AtomicU8,
    consecutive_failures: AtomicU64,
    last_failure_time_nanos: AtomicU64,
    config: CircuitBreakerConfig,
    /// Reference epoch for timestamp calculations
    epoch: Instant,
}

impl CircuitBreaker {
    /// Create a new circuit breaker with default configuration.
    pub fn new() -> Self {
        Self::with_config(CircuitBreakerConfig::default())
    }

    /// Create a new circuit breaker with custom configuration.
    pub fn with_config(config: CircuitBreakerConfig) -> Self {
        Self {
            state: AtomicU8::new(CircuitState::Closed as u8),
            consecutive_failures: AtomicU64::new(0),
            last_failure_time_nanos: AtomicU64::new(0),
            config,
            epoch: Instant::now(),
        }
    }

    /// Get the current circuit state.
    pub fn state(&self) -> CircuitState {
        CircuitState::from(self.state.load(Ordering::Acquire))
    }

    /// Check if the circuit should allow an operation.
    ///
    /// Returns `true` if the operation should proceed, `false` if it should fail-open.
    pub fn allow_request(&self) -> bool {
        match self.state() {
            CircuitState::Closed => true,
            CircuitState::Open => {
                // Check if recovery timeout has elapsed
                let now = Instant::now();
                let last_failure = self.last_failure_time();

                if now.duration_since(last_failure) >= self.config.recovery_timeout {
                    // Attempt recovery by transitioning to half-open using compare-exchange
                    // This ensures only one thread transitions to HalfOpen
                    let result = self.state.compare_exchange(
                        CircuitState::Open as u8,
                        CircuitState::HalfOpen as u8,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    );

                    // Return true only if we successfully transitioned or if already HalfOpen
                    result.is_ok() || self.state() == CircuitState::HalfOpen
                } else {
                    // Circuit still open, fail open (allow request)
                    false
                }
            }
            CircuitState::HalfOpen => {
                // In half-open, allow one test request
                true
            }
        }
    }

    /// Record a successful operation.
    pub fn record_success(&self) {
        let current_state = self.state();

        match current_state {
            CircuitState::HalfOpen => {
                // Recovery successful, close circuit
                self.consecutive_failures.store(0, Ordering::Release);
                self.state
                    .store(CircuitState::Closed as u8, Ordering::Release);
            }
            CircuitState::Closed => {
                // Reset failure count on success
                self.consecutive_failures.store(0, Ordering::Release);
            }
            CircuitState::Open => {
                // Shouldn't happen, but handle gracefully
            }
        }
    }

    /// Record a failed operation.
    pub fn record_failure(&self) {
        let failures = self.consecutive_failures.fetch_add(1, Ordering::Relaxed) + 1;

        // Update last failure time
        let now = Instant::now();
        let nanos = now
            .duration_since(self.epoch)
            .as_nanos()
            .try_into()
            .unwrap_or(u64::MAX);
        self.last_failure_time_nanos.store(nanos, Ordering::Release);

        let current_state = self.state();

        match current_state {
            CircuitState::HalfOpen => {
                // Recovery failed, reopen circuit
                self.state
                    .store(CircuitState::Open as u8, Ordering::Release);
            }
            CircuitState::Closed => {
                // Check if we've hit threshold
                if failures >= self.config.failure_threshold as u64 {
                    self.state
                        .store(CircuitState::Open as u8, Ordering::Release);
                }
            }
            CircuitState::Open => {
                // Already open, stay open
            }
        }
    }

    /// Get the last failure time.
    fn last_failure_time(&self) -> Instant {
        let nanos = self.last_failure_time_nanos.load(Ordering::Acquire);
        self.epoch + Duration::from_nanos(nanos)
    }

    /// Get the number of consecutive failures.
    pub fn consecutive_failures(&self) -> u64 {
        self.consecutive_failures.load(Ordering::Relaxed)
    }

    /// Reset the circuit breaker to closed state.
    pub fn reset(&self) {
        self.state
            .store(CircuitState::Closed as u8, Ordering::Release);
        self.consecutive_failures.store(0, Ordering::Release);
    }
}

impl Default for CircuitBreaker {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for CircuitBreaker {
    fn clone(&self) -> Self {
        Self {
            state: AtomicU8::new(self.state.load(Ordering::Relaxed)),
            consecutive_failures: AtomicU64::new(self.consecutive_failures.load(Ordering::Relaxed)),
            last_failure_time_nanos: AtomicU64::new(
                self.last_failure_time_nanos.load(Ordering::Relaxed),
            ),
            config: self.config.clone(),
            epoch: self.epoch,
        }
    }
}

/// Shareable circuit breaker reference.
pub type SharedCircuitBreaker = Arc<CircuitBreaker>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn test_initial_state() {
        let cb = CircuitBreaker::new();
        assert_eq!(cb.state(), CircuitState::Closed);
        assert_eq!(cb.consecutive_failures(), 0);
        assert!(cb.allow_request());
    }

    #[test]
    fn test_failure_threshold() {
        let config = CircuitBreakerConfig {
            failure_threshold: 3,
            recovery_timeout: Duration::from_secs(1),
        };
        let cb = CircuitBreaker::with_config(config);

        // Record failures below threshold
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Closed);
        assert_eq!(cb.consecutive_failures(), 1);

        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Closed);
        assert_eq!(cb.consecutive_failures(), 2);

        // Third failure should open circuit
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
        assert_eq!(cb.consecutive_failures(), 3);
    }

    #[test]
    fn test_fail_open_when_circuit_open() {
        let config = CircuitBreakerConfig {
            failure_threshold: 2,
            recovery_timeout: Duration::from_secs(10),
        };
        let cb = CircuitBreaker::with_config(config);

        // Open circuit
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);

        // Should fail open (return false to allow request through)
        assert!(!cb.allow_request());
    }

    #[test]
    fn test_recovery_after_timeout() {
        let config = CircuitBreakerConfig {
            failure_threshold: 2,
            recovery_timeout: Duration::from_millis(100),
        };
        let cb = CircuitBreaker::with_config(config);

        // Open circuit
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);

        // Wait for recovery timeout
        thread::sleep(Duration::from_millis(150));

        // Should transition to half-open
        assert!(cb.allow_request());
        assert_eq!(cb.state(), CircuitState::HalfOpen);
    }

    #[test]
    fn test_half_open_success_closes_circuit() {
        let config = CircuitBreakerConfig {
            failure_threshold: 2,
            recovery_timeout: Duration::from_millis(100),
        };
        let cb = CircuitBreaker::with_config(config);

        // Open circuit
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);

        // Wait for recovery
        thread::sleep(Duration::from_millis(150));
        cb.allow_request(); // Transition to half-open

        // Record success should close circuit
        cb.record_success();
        assert_eq!(cb.state(), CircuitState::Closed);
        assert_eq!(cb.consecutive_failures(), 0);
    }

    #[test]
    fn test_half_open_failure_reopens_circuit() {
        let config = CircuitBreakerConfig {
            failure_threshold: 2,
            recovery_timeout: Duration::from_millis(100),
        };
        let cb = CircuitBreaker::with_config(config);

        // Open circuit
        cb.record_failure();
        cb.record_failure();

        // Wait for recovery
        thread::sleep(Duration::from_millis(150));
        cb.allow_request(); // Transition to half-open

        // Record failure should reopen circuit
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
    }

    #[test]
    fn test_success_resets_failure_count() {
        let cb = CircuitBreaker::new();

        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.consecutive_failures(), 2);

        cb.record_success();
        assert_eq!(cb.consecutive_failures(), 0);
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn test_reset() {
        let config = CircuitBreakerConfig {
            failure_threshold: 2,
            recovery_timeout: Duration::from_secs(10),
        };
        let cb = CircuitBreaker::with_config(config);

        // Open circuit
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);

        // Reset should close circuit
        cb.reset();
        assert_eq!(cb.state(), CircuitState::Closed);
        assert_eq!(cb.consecutive_failures(), 0);
        assert!(cb.allow_request());
    }

    #[test]
    fn test_concurrent_failures() {
        let cb = Arc::new(CircuitBreaker::new());
        let mut handles = vec![];

        // Spawn 10 threads recording failures concurrently
        for _ in 0..10 {
            let cb_clone = Arc::clone(&cb);
            handles.push(thread::spawn(move || {
                cb_clone.record_failure();
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        // Should have 10 failures and circuit should be open
        assert_eq!(cb.consecutive_failures(), 10);
        assert_eq!(cb.state(), CircuitState::Open);
    }

    #[test]
    fn test_clone() {
        let cb1 = CircuitBreaker::new();
        cb1.record_failure();

        let cb2 = cb1.clone();

        // Clone should have same failure count
        assert_eq!(cb2.consecutive_failures(), 1);

        // But they're independent
        cb2.record_failure();
        assert_eq!(cb1.consecutive_failures(), 1);
        assert_eq!(cb2.consecutive_failures(), 2);
    }

    #[test]
    fn test_concurrent_half_open_to_closed_race() {
        let config = CircuitBreakerConfig {
            failure_threshold: 2,
            recovery_timeout: Duration::from_millis(100),
        };
        let cb = Arc::new(CircuitBreaker::with_config(config));

        // Open the circuit
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);

        // Wait for recovery timeout
        thread::sleep(Duration::from_millis(150));

        // Trigger state check to transition to HalfOpen
        cb.allow_request();
        assert_eq!(cb.state(), CircuitState::HalfOpen);

        // Multiple threads racing to transition from HalfOpen to Closed
        let mut handles = vec![];
        for _ in 0..10 {
            let cb_clone = Arc::clone(&cb);
            handles.push(thread::spawn(move || {
                cb_clone.record_success();
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        // Should be closed after successful transitions
        assert_eq!(cb.state(), CircuitState::Closed);
        // Consecutive failures should be reset to 0
        assert_eq!(cb.consecutive_failures(), 0);
    }

    #[test]
    fn test_concurrent_half_open_to_open_race() {
        let config = CircuitBreakerConfig {
            failure_threshold: 2,
            recovery_timeout: Duration::from_millis(100),
        };
        let cb = Arc::new(CircuitBreaker::with_config(config));

        // Open the circuit
        cb.record_failure();
        cb.record_failure();

        // Wait for recovery
        thread::sleep(Duration::from_millis(150));

        // Trigger state check to transition to HalfOpen
        cb.allow_request();
        assert_eq!(cb.state(), CircuitState::HalfOpen);

        // Multiple threads racing to record failures
        let mut handles = vec![];
        for _ in 0..10 {
            let cb_clone = Arc::clone(&cb);
            handles.push(thread::spawn(move || {
                cb_clone.record_failure();
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        // Should be back to open
        assert_eq!(cb.state(), CircuitState::Open);
        assert!(cb.consecutive_failures() >= 2);
    }

    #[test]
    fn test_time_going_backwards() {
        // This tests that the circuit breaker handles system clock adjustments gracefully
        let config = CircuitBreakerConfig {
            failure_threshold: 2,
            recovery_timeout: Duration::from_millis(100),
        };
        let cb = CircuitBreaker::with_config(config);

        // Open the circuit
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);

        // In a real scenario, if system time goes backwards (NTP correction),
        // saturating_duration_since will return Duration::ZERO
        // The circuit should remain open until real time passes recovery_timeout

        // We can't actually make time go backwards in tests, but we can verify
        // the circuit doesn't panic with edge case durations
        thread::sleep(Duration::from_millis(150));

        // Trigger state check to transition to HalfOpen
        cb.allow_request();
        assert_eq!(cb.state(), CircuitState::HalfOpen);
    }

    #[test]
    fn test_rapid_state_transitions() {
        let config = CircuitBreakerConfig {
            failure_threshold: 2,
            recovery_timeout: Duration::from_millis(100),
        };
        let cb = Arc::new(CircuitBreaker::with_config(config));

        // Rapidly cycle through states
        for _ in 0..100 {
            cb.record_failure();
            cb.record_failure(); // Open
            thread::sleep(Duration::from_millis(150));
            cb.allow_request(); // Trigger transition to HalfOpen
            cb.record_success(); // Closed
        }

        // Should end in Closed state with no failures
        assert_eq!(cb.state(), CircuitState::Closed);
        assert_eq!(cb.consecutive_failures(), 0);
    }

    #[test]
    fn test_concurrent_mixed_operations() {
        let cb = Arc::new(CircuitBreaker::new());
        let mut handles = vec![];

        // Thread 1: Records failures
        let cb1 = Arc::clone(&cb);
        handles.push(thread::spawn(move || {
            for _ in 0..20 {
                cb1.record_failure();
                thread::sleep(Duration::from_micros(100));
            }
        }));

        // Thread 2: Records successes
        let cb2 = Arc::clone(&cb);
        handles.push(thread::spawn(move || {
            for _ in 0..20 {
                cb2.record_success();
                thread::sleep(Duration::from_micros(100));
            }
        }));

        // Thread 3: Checks allow_request
        let cb3 = Arc::clone(&cb);
        handles.push(thread::spawn(move || {
            for _ in 0..20 {
                let _allowed = cb3.allow_request();
                thread::sleep(Duration::from_micros(100));
            }
        }));

        // Thread 4: Reads state
        let cb4 = Arc::clone(&cb);
        handles.push(thread::spawn(move || {
            for _ in 0..20 {
                let _state = cb4.state();
                let _failures = cb4.consecutive_failures();
                thread::sleep(Duration::from_micros(100));
            }
        }));

        for handle in handles {
            handle.join().unwrap();
        }

        // No assertions on final state - test just verifies no panics or data corruption
    }

    #[test]
    fn test_state_consistency_after_clone() {
        let config = CircuitBreakerConfig {
            failure_threshold: 2,
            recovery_timeout: Duration::from_millis(100),
        };
        let cb1 = CircuitBreaker::with_config(config);
        cb1.record_failure();

        let cb2 = cb1.clone();

        // Both should be in Closed state (not enough failures to open)
        assert_eq!(cb1.state(), CircuitState::Closed);
        assert_eq!(cb2.state(), CircuitState::Closed);

        // One more failure on cb2 should open it (cb2 has 2 failures total)
        cb2.record_failure();

        // cb2 should be Open (has 2 failures)
        assert_eq!(cb2.consecutive_failures(), 2);
        assert_eq!(cb2.state(), CircuitState::Open);

        // cb1 should still be Closed (only has 1 failure - they have independent state after clone)
        assert_eq!(cb1.state(), CircuitState::Closed);
    }

    #[test]
    fn test_consecutive_failures_overflow_resistance() {
        let cb = CircuitBreaker::new();

        // Record massive number of failures
        for _ in 0..10_000 {
            cb.record_failure();
        }

        // Should handle large failure counts without overflow
        assert!(cb.consecutive_failures() >= 10_000);
        assert_eq!(cb.state(), CircuitState::Open);
    }

    #[test]
    fn test_recovery_timeout_boundary() {
        let cb = CircuitBreaker::with_config(CircuitBreakerConfig {
            failure_threshold: 1,
            recovery_timeout: Duration::from_millis(50),
        });

        // Open circuit
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);

        // Before timeout - should still be Open
        thread::sleep(Duration::from_millis(30));
        assert_eq!(cb.state(), CircuitState::Open);

        // After timeout - should transition to HalfOpen when checked
        thread::sleep(Duration::from_millis(30)); // Total 60ms > 50ms timeout
        cb.allow_request(); // Trigger transition to HalfOpen
        assert_eq!(cb.state(), CircuitState::HalfOpen);
    }

    #[test]
    fn test_allow_request_consistency() {
        let config = CircuitBreakerConfig {
            failure_threshold: 2,
            recovery_timeout: Duration::from_millis(100),
        };
        let cb = CircuitBreaker::with_config(config);

        // Closed: should allow
        assert!(cb.allow_request());

        // Open: should not allow
        cb.record_failure();
        cb.record_failure();
        assert!(!cb.allow_request());
        assert!(!cb.allow_request());

        // HalfOpen: should allow (gives it a chance)
        thread::sleep(Duration::from_millis(150));
        assert!(cb.allow_request());
    }
}
