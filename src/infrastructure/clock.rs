//! Clock adapters for time operations.
//!
//! Provides SystemClock implementation for production use.
//!
//! # Testing
//!
//! See `MockClock` (in `crate::infrastructure::mocks`) for a controllable test clock.
//! Available with the `test-helpers` feature or in test builds:
//!
//! ```toml
//! [dev-dependencies]
//! tracing-throttle = { version = "*", features = ["test-helpers"] }
//! ```

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
}
