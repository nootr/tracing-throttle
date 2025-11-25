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
use tokio::{sync::watch, time::interval};

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

/// Handle for controlling a running emitter task.
///
/// # Shutdown Behavior
///
/// You **must** call `shutdown().await` to stop the emitter task. The handle does
/// not implement `Drop` to avoid race conditions and resource leaks.
///
/// If you drop the handle without calling `shutdown()`, the background task will
/// continue running indefinitely, potentially causing:
/// - Resource leaks (the task holds references to the registry)
/// - Unexpected behavior if the task outlives expected lifetime
/// - Inability to observe task failures or panics
///
/// # Examples
///
/// ```rust,no_run
/// # use tracing_throttle::application::emitter::{SummaryEmitter, EmitterConfig};
/// # use tracing_throttle::application::registry::SuppressionRegistry;
/// # use tracing_throttle::domain::policy::Policy;
/// # use tracing_throttle::infrastructure::storage::ShardedStorage;
/// # use tracing_throttle::infrastructure::clock::SystemClock;
/// # use std::sync::Arc;
/// # async fn example() {
/// # let storage = Arc::new(ShardedStorage::new());
/// # let clock = Arc::new(SystemClock::new());
/// # let policy = Policy::count_based(100).unwrap();
/// # let registry = SuppressionRegistry::new(storage, clock, policy);
/// # let config = EmitterConfig::default();
/// # let emitter = SummaryEmitter::new(registry, config);
/// let handle = emitter.start(|_| {}, false);
///
/// // Always call shutdown explicitly
/// handle.shutdown().await;
/// # }
/// ```
#[cfg(feature = "async")]
pub struct EmitterHandle {
    shutdown_tx: watch::Sender<bool>,
    join_handle: Option<tokio::task::JoinHandle<()>>,
}

#[cfg(feature = "async")]
impl EmitterHandle {
    /// Trigger graceful shutdown and wait for the task to complete.
    ///
    /// This method:
    /// 1. Sends the shutdown signal
    /// 2. Waits for the background task to finish
    /// 3. Returns when shutdown is complete
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use tracing_throttle::application::emitter::{SummaryEmitter, EmitterConfig};
    /// # use tracing_throttle::application::registry::SuppressionRegistry;
    /// # use tracing_throttle::domain::policy::Policy;
    /// # use tracing_throttle::infrastructure::storage::ShardedStorage;
    /// # use tracing_throttle::infrastructure::clock::SystemClock;
    /// # use std::sync::Arc;
    /// # async fn example() {
    /// # let storage = Arc::new(ShardedStorage::new());
    /// # let clock = Arc::new(SystemClock::new());
    /// # let policy = Policy::count_based(100).unwrap();
    /// # let registry = SuppressionRegistry::new(storage, clock, policy);
    /// # let config = EmitterConfig::default();
    /// # let emitter = SummaryEmitter::new(registry, config);
    /// let handle = emitter.start(|_| {}, false);
    ///
    /// // Do some work...
    ///
    /// // Clean shutdown
    /// handle.shutdown().await;
    /// # }
    /// ```
    pub async fn shutdown(mut self) {
        // Send shutdown signal
        if self.shutdown_tx.send(true).is_err() {
            // This only fails if all receivers are dropped, which shouldn't happen
            // in normal operation since we hold the receiver in the spawned task
            #[cfg(debug_assertions)]
            eprintln!("Warning: Failed to send shutdown signal - task may have already exited");
        }

        // Wait for task to complete
        if let Some(handle) = self.join_handle.take() {
            match handle.await {
                Ok(()) => {
                    // Clean shutdown
                }
                Err(e) if e.is_panic() => {
                    // Task panicked during shutdown
                    #[cfg(debug_assertions)]
                    eprintln!("Warning: Emitter task panicked during shutdown");
                }
                Err(e) if e.is_cancelled() => {
                    // Task was cancelled (shouldn't happen in normal shutdown)
                    #[cfg(debug_assertions)]
                    eprintln!("Warning: Emitter task was cancelled");
                }
                Err(_) => {
                    // Other error
                    #[cfg(debug_assertions)]
                    eprintln!("Warning: Emitter task failed during shutdown");
                }
            }
        }
    }

    /// Check if the emitter task is still running.
    pub fn is_running(&self) -> bool {
        self.join_handle.as_ref().is_some_and(|h| !h.is_finished())
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
    /// The task will run until `shutdown()` is called on the returned `EmitterHandle`.
    ///
    /// # Graceful Shutdown
    ///
    /// When `shutdown()` is called on the `EmitterHandle`:
    /// 1. The shutdown signal is prioritized over tick events
    /// 2. The current emission completes if in progress
    /// 3. If `emit_final` is true, one final emission occurs with current summaries
    /// 4. The background task completes gracefully
    ///
    /// # Cancellation Safety
    ///
    /// The spawned task is cancellation-safe:
    /// - `collect_summaries()` reads atomically from storage without mutations
    /// - If cancelled during emission, the next startup will see correct state
    /// - Panics in `emit_fn` are caught and don't abort the task
    /// - The `emit_fn` closure should be cancellation-safe (avoid holding locks across `.await`)
    ///
    /// # Type Parameters
    ///
    /// * `F` - The emission function. Must be `Send + 'static` because it runs
    ///   in a spawned task that may execute on any thread in the tokio runtime.
    ///   The function receives ownership of the summaries vector.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use tracing_throttle::application::emitter::{SummaryEmitter, EmitterConfig};
    /// # use tracing_throttle::application::registry::SuppressionRegistry;
    /// # use tracing_throttle::domain::policy::Policy;
    /// # use tracing_throttle::infrastructure::storage::ShardedStorage;
    /// # use tracing_throttle::infrastructure::clock::SystemClock;
    /// # use std::sync::Arc;
    /// # use std::time::Duration;
    /// # async fn example() {
    /// # let storage = Arc::new(ShardedStorage::new());
    /// # let clock = Arc::new(SystemClock::new());
    /// # let policy = Policy::count_based(100).unwrap();
    /// # let registry = SuppressionRegistry::new(storage, clock, policy);
    /// # let config = EmitterConfig::default();
    /// let emitter = SummaryEmitter::new(registry, config);
    /// let handle = emitter.start(|summaries| {
    ///     for summary in summaries {
    ///         tracing::warn!("{}", summary.format_message());
    ///     }
    /// }, true);
    ///
    /// // Later, trigger graceful shutdown
    /// handle.shutdown().await;
    /// # }
    /// ```
    #[cfg(feature = "async")]
    pub fn start<F>(self, mut emit_fn: F, emit_final: bool) -> EmitterHandle
    where
        F: FnMut(Vec<SuppressionSummary>) + Send + 'static,
        S: Send + 'static,
    {
        let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);

        let handle = tokio::spawn(async move {
            let mut ticker = interval(self.config.interval);
            // Skip missed ticks to prevent backpressure if emissions are slow
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            loop {
                tokio::select! {
                    // Prioritize shutdown signal to ensure fast shutdown
                    biased;

                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow_and_update() {
                            // Emit final summaries if requested
                            if emit_final {
                                let summaries = self.collect_summaries();
                                if !summaries.is_empty() {
                                    // Panic safety for final emission too
                                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                        emit_fn(summaries);
                                    }));

                                    if result.is_err() {
                                        #[cfg(debug_assertions)]
                                        eprintln!("Warning: emit_fn panicked during final emission");
                                    }
                                }
                            }
                            break;
                        }
                    }
                    _ = ticker.tick() => {
                        let summaries = self.collect_summaries();
                        if !summaries.is_empty() {
                            // Panic safety: catch panics in emit_fn to prevent task abort
                            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                emit_fn(summaries);
                            }));

                            if result.is_err() {
                                // emit_fn panicked - log but continue running
                                // In production, you might want to track this in metrics
                                #[cfg(debug_assertions)]
                                eprintln!("Warning: emit_fn panicked during emission");
                            }
                        }
                    }
                }
            }
        });

        EmitterHandle {
            shutdown_tx,
            join_handle: Some(handle),
        }
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

        let handle = emitter.start(
            move |summaries| {
                emissions_clone.lock().unwrap().push(summaries.len());
            },
            false,
        );

        // Wait for a couple of intervals
        tokio::time::sleep(Duration::from_millis(250)).await;

        handle.shutdown().await;

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

    #[cfg(feature = "async")]
    #[tokio::test]
    async fn test_graceful_shutdown() {
        use std::sync::Mutex;

        let storage = Arc::new(ShardedStorage::new());
        let clock = Arc::new(SystemClock::new());
        let policy = Policy::count_based(100).unwrap();
        let registry = SuppressionRegistry::new(storage, clock, policy);
        let config = EmitterConfig::new(Duration::from_millis(100)).unwrap();

        // Add a suppressed event so there's something to emit
        let sig = EventSignature::simple("INFO", "Test");
        registry.with_event_state(sig, |state, now| {
            state.counter.record_suppression(now);
        });

        let emitter = SummaryEmitter::new(registry, config);

        let emissions = Arc::new(Mutex::new(0));
        let emissions_clone = Arc::clone(&emissions);

        let handle = emitter.start(
            move |_| {
                *emissions_clone.lock().unwrap() += 1;
            },
            false,
        );

        // Let it run for a bit
        tokio::time::sleep(Duration::from_millis(250)).await;

        // Trigger graceful shutdown
        handle.shutdown().await;

        // Verify task is no longer running
        let final_count = *emissions.lock().unwrap();
        assert!(final_count >= 1);

        // Wait a bit more to ensure task really stopped
        tokio::time::sleep(Duration::from_millis(150)).await;
        let count_after_shutdown = *emissions.lock().unwrap();
        assert_eq!(count_after_shutdown, final_count);
    }

    #[cfg(feature = "async")]
    #[tokio::test]
    async fn test_shutdown_with_final_emission() {
        use std::sync::Mutex;

        let storage = Arc::new(ShardedStorage::new());
        let clock = Arc::new(SystemClock::new());
        let policy = Policy::count_based(100).unwrap();
        let registry = SuppressionRegistry::new(storage, clock, policy);
        let config = EmitterConfig::new(Duration::from_secs(60)).unwrap(); // Long interval

        let emitter = SummaryEmitter::new(registry.clone(), config);

        let emissions = Arc::new(Mutex::new(Vec::new()));
        let emissions_clone = Arc::clone(&emissions);

        let handle = emitter.start(
            move |summaries| {
                emissions_clone.lock().unwrap().push(summaries.len());
            },
            true, // Emit final summaries
        );

        // Wait for first tick to complete (interval's first tick is immediate)
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Now add some suppressions after the first tick
        let sig = EventSignature::simple("INFO", "Test event");
        registry.with_event_state(sig, |state, now| {
            for _ in 0..10 {
                state.counter.record_suppression(now);
            }
        });

        // Shutdown before next interval (which is 60 seconds away)
        tokio::time::sleep(Duration::from_millis(50)).await;
        handle.shutdown().await;

        // Should have emitted final summaries
        let emission_list = emissions.lock().unwrap();
        assert_eq!(emission_list.len(), 1);
        assert_eq!(emission_list[0], 1); // 1 summary
    }

    #[cfg(feature = "async")]
    #[tokio::test]
    async fn test_shutdown_without_final_emission() {
        use std::sync::Mutex;

        let storage = Arc::new(ShardedStorage::new());
        let clock = Arc::new(SystemClock::new());
        let policy = Policy::count_based(100).unwrap();
        let registry = SuppressionRegistry::new(storage, clock, policy);
        let config = EmitterConfig::new(Duration::from_secs(60)).unwrap();

        let emitter = SummaryEmitter::new(registry.clone(), config);

        let emissions = Arc::new(Mutex::new(0));
        let emissions_clone = Arc::clone(&emissions);

        let handle = emitter.start(
            move |_| {
                *emissions_clone.lock().unwrap() += 1;
            },
            false, // No final emission
        );

        // Wait for first tick (immediate, but no emissions since no suppressions yet)
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Add some suppressions after first tick
        let sig = EventSignature::simple("INFO", "Test event");
        registry.with_event_state(sig, |state, now| {
            state.counter.record_suppression(now);
        });

        // Shutdown immediately (before next 60-second interval)
        tokio::time::sleep(Duration::from_millis(50)).await;
        handle.shutdown().await;

        // Should not have emitted anything (no final emission)
        assert_eq!(*emissions.lock().unwrap(), 0);
    }

    #[cfg(feature = "async")]
    #[tokio::test]
    async fn test_is_running() {
        let storage = Arc::new(ShardedStorage::new());
        let clock = Arc::new(SystemClock::new());
        let policy = Policy::count_based(100).unwrap();
        let registry = SuppressionRegistry::new(storage, clock, policy);
        let config = EmitterConfig::new(Duration::from_millis(100)).unwrap();

        let emitter = SummaryEmitter::new(registry, config);
        let handle = emitter.start(|_| {}, false);

        // Should be running
        assert!(handle.is_running());

        // Shutdown
        handle.shutdown().await;

        // Should no longer be running
        tokio::time::sleep(Duration::from_millis(50)).await;
        // Note: is_running() consumes self, so we can't check after shutdown
    }

    #[cfg(feature = "async")]
    #[tokio::test]
    async fn test_shutdown_during_emission() {
        use std::sync::{Arc, Mutex};

        let storage = Arc::new(ShardedStorage::new());
        let clock = Arc::new(SystemClock::new());
        let policy = Policy::count_based(100).unwrap();
        let registry = SuppressionRegistry::new(storage, clock, policy);
        let config = EmitterConfig::new(Duration::from_millis(50)).unwrap();

        // Add suppressions
        let sig = EventSignature::simple("INFO", "Test");
        registry.with_event_state(sig, |state, now| {
            state.counter.record_suppression(now);
        });

        let emitter = SummaryEmitter::new(registry, config);

        let emissions = Arc::new(Mutex::new(0));
        let emissions_clone = Arc::clone(&emissions);

        let handle = emitter.start(
            move |_| {
                // Simulate slow emission
                std::thread::sleep(Duration::from_millis(30));
                *emissions_clone.lock().unwrap() += 1;
            },
            false,
        );

        // Let first emission start
        tokio::time::sleep(Duration::from_millis(60)).await;

        // Shutdown should wait for current emission to complete
        handle.shutdown().await;

        // Current emission should have completed
        assert!(*emissions.lock().unwrap() >= 1);
    }
}
