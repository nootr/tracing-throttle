//! Mock tracing layer for testing.

use std::sync::{Arc, Mutex};
use tracing::Level;
use tracing_subscriber::Layer;

/// Mock layer that captures events for testing.
#[derive(Clone)]
pub struct MockCaptureLayer {
    captured: Arc<Mutex<Vec<CapturedEvent>>>,
}

/// Captured event information.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct CapturedEvent {
    pub level: Level,
    pub message: String,
}

impl MockCaptureLayer {
    /// Create a new mock capture layer.
    pub fn new() -> Self {
        Self {
            captured: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Get all captured events.
    pub fn get_captured(&self) -> Vec<CapturedEvent> {
        self.captured
            .lock()
            .expect(
                "MockCaptureLayer mutex poisoned - a test thread panicked while holding the lock",
            )
            .clone()
    }

    /// Get the count of captured events.
    pub fn count(&self) -> usize {
        self.captured
            .lock()
            .expect(
                "MockCaptureLayer mutex poisoned - a test thread panicked while holding the lock",
            )
            .len()
    }

    /// Clear all captured events.
    ///
    /// Useful for resetting state between test cases or managing memory in long-running tests.
    ///
    /// # Examples
    ///
    /// ```
    /// use tracing_throttle::infrastructure::mocks::MockCaptureLayer;
    /// use tracing::info;
    /// use tracing_subscriber::layer::SubscriberExt;
    ///
    /// let capture = MockCaptureLayer::new();
    /// let subscriber = tracing_subscriber::registry().with(capture.clone());
    ///
    /// tracing::subscriber::with_default(subscriber, || {
    ///     info!("test message");
    ///     assert_eq!(capture.count(), 1);
    ///
    ///     capture.clear();
    ///     assert_eq!(capture.count(), 0);
    /// });
    /// ```
    pub fn clear(&self) {
        self.captured
            .lock()
            .expect(
                "MockCaptureLayer mutex poisoned - a test thread panicked while holding the lock",
            )
            .clear();
    }
}

impl Default for MockCaptureLayer {
    fn default() -> Self {
        Self::new()
    }
}

impl<S> Layer<S> for MockCaptureLayer
where
    S: tracing::Subscriber,
{
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let mut visitor = EventVisitor {
            message: String::new(),
        };
        event.record(&mut visitor);

        self.captured
            .lock()
            .expect(
                "MockCaptureLayer mutex poisoned - a test thread panicked while holding the lock",
            )
            .push(CapturedEvent {
                level: *event.metadata().level(),
                message: visitor.message,
            });
    }
}

struct EventVisitor {
    message: String,
}

impl tracing::field::Visit for EventVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{:?}", value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing::info;
    use tracing_subscriber::layer::SubscriberExt;

    #[test]
    fn test_mock_capture_layer() {
        let capture = MockCaptureLayer::new();
        let subscriber = tracing_subscriber::registry().with(capture.clone());

        tracing::subscriber::with_default(subscriber, || {
            info!("test message");
        });

        assert_eq!(capture.count(), 1);
        let events = capture.get_captured();
        assert_eq!(events[0].level, Level::INFO);
    }
}
