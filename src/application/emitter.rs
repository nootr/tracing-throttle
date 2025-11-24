//! Summary emission for suppressed events.
//!
//! Periodically collects and emits summaries of suppressed events to provide
//! visibility into what has been rate limited.

use crate::application::{
    ports::Storage,
    registry::{EventState, SuppressionRegistry},
};
use crate::domain::{signature::EventSignature, summary::SuppressionSummary};
use std::time::Duration;

#[cfg(feature = "async")]
use tokio::time::interval;

/// Error returned when emitter configuration validation fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmitterConfigError {
    /// Summary interval duration must be greater than zero
    ZeroSummaryInterval,
}

impl std::fmt::Display for EmitterConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EmitterConfigError::ZeroSummaryInterval => {
                write!(f, "summary interval must be greater than 0")
            }
        }
    }
}

impl std::error::Error for EmitterConfigError {}

/// Configuration for summary emission.
#[derive(Debug, Clone)]
pub struct EmitterConfig {
    /// How often to emit summaries
    pub interval: Duration,
    /// Minimum suppression count to include in summary
    pub min_count: usize,
}

impl Default for EmitterConfig {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(30),
            min_count: 1,
        }
    }
}

impl EmitterConfig {
    /// Create a new emitter config with the specified interval.
    ///
    /// # Errors
    /// Returns `EmitterConfigError::ZeroSummaryInterval` if `interval` is zero.
    pub fn new(interval: Duration) -> Result<Self, EmitterConfigError> {
        if interval.is_zero() {
            return Err(EmitterConfigError::ZeroSummaryInterval);
        }
        Ok(Self {
            interval,
            min_count: 1,
        })
    }

    /// Set the minimum suppression count threshold.
    pub fn with_min_count(mut self, min_count: usize) -> Self {
        self.min_count = min_count;
        self
    }
}

/// Emits periodic summaries of suppressed events.
pub struct SummaryEmitter<S>
where
    S: Storage<EventSignature, EventState> + Clone,
{
    registry: SuppressionRegistry<S>,
    config: EmitterConfig,
}

impl<S> SummaryEmitter<S>
where
    S: Storage<EventSignature, EventState> + Clone,
{
    /// Create a new summary emitter.
    pub fn new(registry: SuppressionRegistry<S>, config: EmitterConfig) -> Self {
        Self { registry, config }
    }

    /// Collect current suppression summaries.
    ///
    /// Returns summaries for all events that have been suppressed at least
    /// `min_count` times.
    pub fn collect_summaries(&self) -> Vec<SuppressionSummary> {
        let mut summaries = Vec::new();
        let min_count = self.config.min_count;

        self.registry.for_each(|signature, state| {
            let count = state.counter.count();

            if count >= min_count {
                let summary = SuppressionSummary::from_counter(*signature, &state.counter);
                summaries.push(summary);
            }
        });

        summaries
    }

    /// Start emitting summaries periodically (async version).
    ///
    /// This spawns a background task that emits summaries at the configured interval.
    #[cfg(feature = "async")]
    pub fn start<F>(self, mut emit_fn: F) -> tokio::task::JoinHandle<()>
    where
        F: FnMut(Vec<SuppressionSummary>) + Send + 'static,
        S: Send + 'static,
    {
        tokio::spawn(async move {
            let mut ticker = interval(self.config.interval);

            loop {
                ticker.tick().await;
                let summaries = self.collect_summaries();

                if !summaries.is_empty() {
                    emit_fn(summaries);
                }
            }
        })
    }

    /// Get the emitter configuration.
    pub fn config(&self) -> &EmitterConfig {
        &self.config
    }

    /// Get a reference to the registry.
    pub fn registry(&self) -> &SuppressionRegistry<S> {
        &self.registry
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{policy::Policy, signature::EventSignature};
    use crate::infrastructure::clock::SystemClock;
    use crate::infrastructure::storage::ShardedStorage;
    use std::sync::Arc;

    #[test]
    fn test_collect_summaries_empty() {
        let storage = Arc::new(ShardedStorage::new());
        let clock = Arc::new(SystemClock::new());
        let policy = Policy::count_based(100).unwrap();
        let registry = SuppressionRegistry::new(storage, clock, policy);
        let config = EmitterConfig::default();
        let emitter = SummaryEmitter::new(registry, config);

        let summaries = emitter.collect_summaries();
        assert!(summaries.is_empty());
    }

    #[test]
    fn test_collect_summaries_with_suppressions() {
        let storage = Arc::new(ShardedStorage::new());
        let clock = Arc::new(SystemClock::new());
        let policy = Policy::count_based(100).unwrap();
        let registry = SuppressionRegistry::new(storage, clock, policy);
        let config = EmitterConfig::default();

        // Add some suppressed events
        for i in 0..3 {
            let sig = EventSignature::simple("INFO", &format!("Message {}", i));
            registry.with_event_state(sig, |state, now| {
                // Simulate some suppressions
                for _ in 0..(i + 1) * 5 {
                    state.counter.record_suppression(now);
                }
            });
        }

        let emitter = SummaryEmitter::new(registry, config);
        let summaries = emitter.collect_summaries();

        assert_eq!(summaries.len(), 3);

        // Verify counts
        let counts: Vec<usize> = summaries.iter().map(|s| s.count).collect();
        assert!(counts.contains(&6)); // 5 + 1 (initial)
        assert!(counts.contains(&11)); // 10 + 1
        assert!(counts.contains(&16)); // 15 + 1
    }

    #[test]
    fn test_min_count_filtering() {
        let storage = Arc::new(ShardedStorage::new());
        let clock = Arc::new(SystemClock::new());
        let policy = Policy::count_based(100).unwrap();
        let registry = SuppressionRegistry::new(storage, clock, policy);
        let config = EmitterConfig::default().with_min_count(10);

        // Add event with low count (below threshold)
        let sig1 = EventSignature::simple("INFO", "Low count");
        registry.with_event_state(sig1, |state, now| {
            for _ in 0..4 {
                state.counter.record_suppression(now);
            }
        });

        // Add event with high count (above threshold)
        let sig2 = EventSignature::simple("INFO", "High count");
        registry.with_event_state(sig2, |state, now| {
            for _ in 0..14 {
                state.counter.record_suppression(now);
            }
        });

        let emitter = SummaryEmitter::new(registry, config);
        let summaries = emitter.collect_summaries();

        // Only the high-count event should be included
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].count, 15); // 14 + 1 initial
    }

    #[cfg(feature = "async")]
    #[tokio::test]
    async fn test_async_emission() {
        use std::sync::Mutex;

        let storage = Arc::new(ShardedStorage::new());
        let clock = Arc::new(SystemClock::new());
        let policy = Policy::count_based(100).unwrap();
        let registry = SuppressionRegistry::new(storage, clock, policy);
        let config = EmitterConfig::new(Duration::from_millis(100)).unwrap();

        // Add a suppressed event
        let sig = EventSignature::simple("INFO", "Test");
        registry.with_event_state(sig, |state, now| {
            state.counter.record_suppression(now);
        });

        let emitter = SummaryEmitter::new(registry, config);

        // Track emissions
        let emissions = Arc::new(Mutex::new(Vec::new()));
        let emissions_clone = Arc::clone(&emissions);

        let handle = emitter.start(move |summaries| {
            emissions_clone.lock().unwrap().push(summaries.len());
        });

        // Wait for a couple of intervals
        tokio::time::sleep(Duration::from_millis(250)).await;

        handle.abort();

        // Should have emitted at least once
        let emission_count = emissions.lock().unwrap().len();
        assert!(emission_count >= 2);
    }

    #[test]
    fn test_emitter_config_zero_interval() {
        let result = EmitterConfig::new(Duration::from_secs(0));
        assert!(matches!(
            result,
            Err(EmitterConfigError::ZeroSummaryInterval)
        ));
    }

    #[test]
    fn test_emitter_config_valid_interval() {
        let config = EmitterConfig::new(Duration::from_secs(30)).unwrap();
        assert_eq!(config.interval, Duration::from_secs(30));
        assert_eq!(config.min_count, 1);
    }
}
