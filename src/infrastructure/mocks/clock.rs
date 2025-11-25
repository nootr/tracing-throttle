//! Mock clock for testing.

use crate::application::ports::Clock;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Mock clock for testing.
///
/// Allows tests to control time progression explicitly, enabling deterministic
/// testing of time-based rate limiting policies.
///
/// # Examples
///
/// ```
/// use tracing_throttle::infrastructure::mocks::MockClock;
/// use tracing_throttle::application::ports::Clock;
/// use std::time::{Duration, Instant};
///
/// let start = Instant::now();
/// let clock = MockClock::new(start);
///
/// // Time starts at the specified instant
/// assert_eq!(clock.now(), start);
///
/// // Advance time explicitly
/// clock.advance(Duration::from_secs(10));
/// assert_eq!(clock.now(), start + Duration::from_secs(10));
///
/// // Or set to a specific instant
/// let new_time = start + Duration::from_secs(100);
/// clock.set(new_time);
/// assert_eq!(clock.now(), new_time);
/// ```
///
/// # Thread Safety
///
/// `MockClock` is thread-safe and can be cloned to share across threads.
/// All clones share the same underlying time value, so advancing time in
/// one clone affects all clones.
///
/// ```
/// use tracing_throttle::infrastructure::mocks::MockClock;
/// use tracing_throttle::application::ports::Clock;
/// use std::time::{Duration, Instant};
/// use std::thread;
///
/// let start = Instant::now();
/// let clock = MockClock::new(start);
/// let clock_clone = clock.clone();
///
/// let handle = thread::spawn(move || {
///     clock_clone.advance(Duration::from_secs(5));
/// });
///
/// handle.join().unwrap();
/// assert_eq!(clock.now(), start + Duration::from_secs(5));
/// ```
#[derive(Debug, Clone)]
pub struct MockClock {
    current_time: Arc<Mutex<Instant>>,
}

impl MockClock {
    /// Create a mock clock starting at a specific instant.
    pub fn new(start: Instant) -> Self {
        Self {
            current_time: Arc::new(Mutex::new(start)),
        }
    }

    /// Advance the clock by a duration.
    pub fn advance(&self, duration: std::time::Duration) {
        let mut time = self
            .current_time
            .lock()
            .expect("MockClock mutex poisoned - a test thread panicked while holding the lock");
        *time += duration;
    }

    /// Set the clock to a specific instant.
    pub fn set(&self, instant: Instant) {
        let mut time = self
            .current_time
            .lock()
            .expect("MockClock mutex poisoned - a test thread panicked while holding the lock");
        *time = instant;
    }
}

impl Clock for MockClock {
    fn now(&self) -> Instant {
        *self
            .current_time
            .lock()
            .expect("MockClock mutex poisoned - a test thread panicked while holding the lock")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_mock_clock() {
        let start = Instant::now();
        let clock = MockClock::new(start);

        assert_eq!(clock.now(), start);

        clock.advance(Duration::from_secs(10));
        assert_eq!(clock.now(), start + Duration::from_secs(10));

        let new_time = start + Duration::from_secs(100);
        clock.set(new_time);
        assert_eq!(clock.now(), new_time);
    }
}
