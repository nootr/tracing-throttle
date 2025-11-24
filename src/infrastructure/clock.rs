//! Clock adapters for time operations.
//!
//! Provides concrete implementations of the Clock port defined in the
//! application layer, enabling both production (SystemClock) and test
//! (MockClock) scenarios.

use crate::application::ports::Clock;
use std::time::Instant;

/// System clock implementation using `Instant::now()`.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl SystemClock {
    /// Create a new system clock.
    pub fn new() -> Self {
        Self
    }
}

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// Mock clock for testing.
///
/// Allows tests to control time progression explicitly.
#[cfg(test)]
#[derive(Debug, Clone)]
pub struct MockClock {
    current_time: std::sync::Arc<std::sync::Mutex<Instant>>,
}

#[cfg(test)]
impl MockClock {
    /// Create a mock clock starting at a specific instant.
    pub fn new(start: Instant) -> Self {
        Self {
            current_time: std::sync::Arc::new(std::sync::Mutex::new(start)),
        }
    }

    /// Advance the clock by a duration.
    pub fn advance(&self, duration: std::time::Duration) {
        let mut time = self.current_time.lock().unwrap();
        *time += duration;
    }

    /// Set the clock to a specific instant.
    pub fn set(&self, instant: Instant) {
        let mut time = self.current_time.lock().unwrap();
        *time = instant;
    }
}

#[cfg(test)]
impl Clock for MockClock {
    fn now(&self) -> Instant {
        *self.current_time.lock().unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_system_clock() {
        let clock = SystemClock::new();
        let t1 = clock.now();
        std::thread::sleep(Duration::from_millis(10));
        let t2 = clock.now();

        assert!(t2 > t1);
    }

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
