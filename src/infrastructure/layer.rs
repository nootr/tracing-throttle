//! Tracing integration layer.
//!
//! Provides a `tracing::Layer` implementation that applies rate limiting
//! to log events.

use crate::application::{
    circuit_breaker::CircuitBreaker,
    emitter::EmitterConfig,
    limiter::{LimitDecision, RateLimiter},
    metrics::Metrics,
    ports::{Clock, Storage},
    registry::{EventState, SuppressionRegistry},
};
use crate::domain::{policy::Policy, signature::EventSignature};
use crate::infrastructure::clock::SystemClock;
use crate::infrastructure::storage::ShardedStorage;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use tracing::{Metadata, Subscriber};
use tracing_subscriber::layer::Filter;
use tracing_subscriber::{layer::Context, Layer};

/// Error returned when building a TracingRateLimitLayer fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildError {
    /// Maximum signatures must be greater than zero
    ZeroMaxSignatures,
    /// Emitter configuration validation failed
    EmitterConfig(crate::application::emitter::EmitterConfigError),
}

impl std::fmt::Display for BuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BuildError::ZeroMaxSignatures => {
                write!(f, "max_signatures must be greater than 0")
            }
            BuildError::EmitterConfig(e) => {
                write!(f, "emitter configuration error: {}", e)
            }
        }
    }
}

impl std::error::Error for BuildError {}

impl From<crate::application::emitter::EmitterConfigError> for BuildError {
    fn from(e: crate::application::emitter::EmitterConfigError) -> Self {
        BuildError::EmitterConfig(e)
    }
}

/// Builder for constructing a `TracingRateLimitLayer`.
#[derive(Debug)]
pub struct TracingRateLimitLayerBuilder {
    policy: Policy,
    summary_interval: Duration,
    clock: Option<Arc<dyn Clock>>,
    max_signatures: Option<usize>,
}

impl TracingRateLimitLayerBuilder {
    /// Set the rate limiting policy.
    pub fn with_policy(mut self, policy: Policy) -> Self {
        self.policy = policy;
        self
    }

    /// Set the summary emission interval.
    ///
    /// The interval will be validated when `build()` is called.
    pub fn with_summary_interval(mut self, interval: Duration) -> Self {
        self.summary_interval = interval;
        self
    }

    /// Set a custom clock (mainly for testing).
    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = Some(clock);
        self
    }

    /// Set the maximum number of unique event signatures to track.
    ///
    /// When this limit is reached, the least recently used signatures will be evicted.
    /// This prevents unbounded memory growth in applications with high signature cardinality.
    ///
    /// Default: 10,000 signatures
    ///
    /// The value will be validated when `build()` is called.
    pub fn with_max_signatures(mut self, max_signatures: usize) -> Self {
        self.max_signatures = Some(max_signatures);
        self
    }

    /// Disable the signature limit, allowing unbounded growth.
    ///
    /// **Warning**: This can lead to unbounded memory usage in applications that generate
    /// many unique event signatures. Only use this if you're certain your application has
    /// bounded signature cardinality or you have external memory monitoring.
    pub fn with_unlimited_signatures(mut self) -> Self {
        self.max_signatures = None;
        self
    }

    /// Build the layer.
    ///
    /// # Errors
    /// Returns `BuildError` if the configuration is invalid.
    pub fn build(self) -> Result<TracingRateLimitLayer, BuildError> {
        // Validate max_signatures if set
        if let Some(max) = self.max_signatures {
            if max == 0 {
                return Err(BuildError::ZeroMaxSignatures);
            }
        }

        // Create shared metrics and circuit breaker
        let metrics = Metrics::new();
        let circuit_breaker = Arc::new(CircuitBreaker::new());

        let clock = self.clock.unwrap_or_else(|| Arc::new(SystemClock::new()));
        let storage = if let Some(max) = self.max_signatures {
            Arc::new(ShardedStorage::with_max_entries(max).with_metrics(metrics.clone()))
        } else {
            Arc::new(ShardedStorage::new().with_metrics(metrics.clone()))
        };
        let registry = SuppressionRegistry::new(storage, clock, self.policy);
        let limiter = RateLimiter::new(registry, metrics.clone(), circuit_breaker);

        // Let EmitterConfig validate the interval
        let emitter_config = EmitterConfig::new(self.summary_interval)?;

        Ok(TracingRateLimitLayer {
            limiter,
            _emitter_config: emitter_config,
        })
    }
}

/// A `tracing::Layer` that applies rate limiting to events.
///
/// This layer intercepts events, computes their signature, and decides
/// whether to allow or suppress them based on the configured policy.
#[derive(Clone)]
pub struct TracingRateLimitLayer<S = Arc<ShardedStorage<EventSignature, EventState>>>
where
    S: Storage<EventSignature, EventState> + Clone,
{
    limiter: RateLimiter<S>,
    _emitter_config: EmitterConfig,
}

impl<S> TracingRateLimitLayer<S>
where
    S: Storage<EventSignature, EventState> + Clone,
{
    /// Compute event signature from tracing metadata and fields.
    fn compute_signature(
        &self,
        metadata: &Metadata,
        _fields: &BTreeMap<String, String>,
    ) -> EventSignature {
        // For MVP, we'll use a simple signature based on level and target
        // In a full implementation, we'd extract and include field values
        let level = metadata.level().as_str();
        let message = metadata.name();
        let target = Some(metadata.target());

        // TODO: Extract actual field values from the event
        // For now, use empty fields map
        let fields = BTreeMap::new();

        EventSignature::new(level, message, &fields, target)
    }

    /// Check if an event should be allowed through.
    pub fn should_allow(&self, signature: EventSignature) -> bool {
        matches!(self.limiter.check_event(signature), LimitDecision::Allow)
    }

    /// Get a reference to the underlying limiter.
    pub fn limiter(&self) -> &RateLimiter<S> {
        &self.limiter
    }

    /// Get a reference to the metrics.
    ///
    /// Returns metrics about rate limiting behavior including:
    /// - Events allowed
    /// - Events suppressed
    /// - Signatures evicted
    pub fn metrics(&self) -> &Metrics {
        self.limiter.metrics()
    }

    /// Get the current number of tracked signatures.
    pub fn signature_count(&self) -> usize {
        self.limiter.registry().len()
    }

    /// Get a reference to the circuit breaker.
    ///
    /// Use this to check the circuit breaker state and health:
    /// - `circuit_breaker().state()` - Current circuit state
    /// - `circuit_breaker().consecutive_failures()` - Failure count
    pub fn circuit_breaker(&self) -> &Arc<CircuitBreaker> {
        self.limiter.circuit_breaker()
    }
}

impl TracingRateLimitLayer<Arc<ShardedStorage<EventSignature, EventState>>> {
    /// Create a builder for configuring the layer.
    ///
    /// Defaults:
    /// - Policy: token bucket (50 burst capacity, 1 token/sec refill rate)
    /// - Max signatures: 10,000 (with LRU eviction)
    /// - Summary interval: 30 seconds
    pub fn builder() -> TracingRateLimitLayerBuilder {
        TracingRateLimitLayerBuilder {
            policy: Policy::token_bucket(50.0, 1.0)
                .expect("default policy with 50 capacity and 1/sec refill is always valid"),
            summary_interval: Duration::from_secs(30),
            clock: None,
            max_signatures: Some(10_000),
        }
    }

    /// Create a layer with default settings.
    ///
    /// Equivalent to `TracingRateLimitLayer::builder().build().unwrap()`.
    ///
    /// Defaults:
    /// - Policy: token bucket (50 burst capacity, 1 token/sec refill rate = 60/min)
    /// - Max signatures: 10,000 (with LRU eviction)
    /// - Summary interval: 30 seconds
    ///
    /// # Panics
    /// This method cannot panic because all default values are valid.
    pub fn new() -> Self {
        Self::builder()
            .build()
            .expect("default configuration is always valid")
    }
}

impl Default for TracingRateLimitLayer<Arc<ShardedStorage<EventSignature, EventState>>> {
    fn default() -> Self {
        Self::new()
    }
}

// Implement the Filter trait for rate limiting
impl<S, Sub> Filter<Sub> for TracingRateLimitLayer<S>
where
    S: Storage<EventSignature, EventState> + Clone,
    Sub: Subscriber,
{
    fn enabled(&self, meta: &Metadata<'_>, _cx: &Context<'_, Sub>) -> bool {
        // Compute signature and check with rate limiter
        let fields = BTreeMap::new();
        let signature = self.compute_signature(meta, &fields);
        self.should_allow(signature)
    }
}

impl<S, Sub> Layer<Sub> for TracingRateLimitLayer<S>
where
    S: Storage<EventSignature, EventState> + Clone + 'static,
    Sub: Subscriber,
{
    // Layer methods can be empty since filtering is handled by the Filter impl
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing::info;
    use tracing_subscriber::layer::SubscriberExt;

    #[test]
    fn test_layer_builder() {
        let layer = TracingRateLimitLayer::builder()
            .with_policy(Policy::count_based(50).unwrap())
            .with_summary_interval(Duration::from_secs(60))
            .build()
            .unwrap();

        assert!(layer.limiter().registry().is_empty());
    }

    #[test]
    fn test_layer_default() {
        let layer = TracingRateLimitLayer::default();
        assert!(layer.limiter().registry().is_empty());
    }

    #[test]
    fn test_signature_computation() {
        let _layer = TracingRateLimitLayer::new();

        // Use a simple signature test without metadata construction
        let sig1 = EventSignature::simple("INFO", "test_event");
        let sig2 = EventSignature::simple("INFO", "test_event");

        // Same inputs should produce same signature
        assert_eq!(sig1, sig2);
    }

    #[test]
    fn test_basic_rate_limiting() {
        let layer = TracingRateLimitLayer::builder()
            .with_policy(Policy::count_based(2).unwrap())
            .build()
            .unwrap();

        let sig = EventSignature::simple("INFO", "test_message");

        // First two should be allowed
        assert!(layer.should_allow(sig));
        assert!(layer.should_allow(sig));

        // Third should be suppressed
        assert!(!layer.should_allow(sig));
    }

    #[test]
    fn test_layer_integration() {
        let layer = TracingRateLimitLayer::builder()
            .with_policy(Policy::count_based(3).unwrap())
            .build()
            .unwrap();

        // Clone for use in subscriber, keep original for checking state
        let layer_for_check = layer.clone();

        let subscriber = tracing_subscriber::registry()
            .with(tracing_subscriber::fmt::layer().with_filter(layer));

        // Test that the layer correctly tracks event signatures
        tracing::subscriber::with_default(subscriber, || {
            // Emit 10 identical events
            for _ in 0..10 {
                info!("test event");
            }
        });

        // After emitting 10 events with the same signature, the layer should have
        // tracked them and only the first 3 should have been marked as allowed
        // The registry should contain one entry for this signature
        assert_eq!(layer_for_check.limiter().registry().len(), 1);
    }

    #[test]
    fn test_layer_suppression_logic() {
        let layer = TracingRateLimitLayer::builder()
            .with_policy(Policy::count_based(3).unwrap())
            .build()
            .unwrap();

        let sig = EventSignature::simple("INFO", "test");

        // Verify the suppression logic works correctly
        let mut allowed_count = 0;
        for _ in 0..10 {
            if layer.should_allow(sig) {
                allowed_count += 1;
            }
        }

        assert_eq!(allowed_count, 3);
    }

    #[test]
    fn test_builder_zero_summary_interval() {
        let result = TracingRateLimitLayer::builder()
            .with_summary_interval(Duration::from_secs(0))
            .build();

        assert!(matches!(
            result,
            Err(BuildError::EmitterConfig(
                crate::application::emitter::EmitterConfigError::ZeroSummaryInterval
            ))
        ));
    }

    #[test]
    fn test_builder_zero_max_signatures() {
        let result = TracingRateLimitLayer::builder()
            .with_max_signatures(0)
            .build();

        assert!(matches!(result, Err(BuildError::ZeroMaxSignatures)));
    }

    #[test]
    fn test_builder_valid_max_signatures() {
        let layer = TracingRateLimitLayer::builder()
            .with_max_signatures(100)
            .build()
            .unwrap();

        assert!(layer.limiter().registry().is_empty());
    }

    #[test]
    fn test_metrics_tracking() {
        let layer = TracingRateLimitLayer::builder()
            .with_policy(Policy::count_based(2).unwrap())
            .build()
            .unwrap();

        let sig = EventSignature::simple("INFO", "test");

        // Check initial metrics
        assert_eq!(layer.metrics().events_allowed(), 0);
        assert_eq!(layer.metrics().events_suppressed(), 0);

        // Allow first two events
        assert!(layer.should_allow(sig));
        assert!(layer.should_allow(sig));

        // Check metrics after allowed events
        assert_eq!(layer.metrics().events_allowed(), 2);
        assert_eq!(layer.metrics().events_suppressed(), 0);

        // Suppress third event
        assert!(!layer.should_allow(sig));

        // Check metrics after suppressed event
        assert_eq!(layer.metrics().events_allowed(), 2);
        assert_eq!(layer.metrics().events_suppressed(), 1);
    }

    #[test]
    fn test_metrics_snapshot() {
        let layer = TracingRateLimitLayer::builder()
            .with_policy(Policy::count_based(3).unwrap())
            .build()
            .unwrap();

        let sig = EventSignature::simple("INFO", "test");

        // Generate some events
        for _ in 0..5 {
            layer.should_allow(sig);
        }

        // Get snapshot
        let snapshot = layer.metrics().snapshot();
        assert_eq!(snapshot.events_allowed, 3);
        assert_eq!(snapshot.events_suppressed, 2);
        assert_eq!(snapshot.total_events(), 5);
        assert!((snapshot.suppression_rate() - 0.4).abs() < f64::EPSILON);
    }

    #[test]
    fn test_signature_count() {
        let layer = TracingRateLimitLayer::builder()
            .with_policy(Policy::count_based(2).unwrap())
            .build()
            .unwrap();

        assert_eq!(layer.signature_count(), 0);

        let sig1 = EventSignature::simple("INFO", "test1");
        let sig2 = EventSignature::simple("INFO", "test2");

        layer.should_allow(sig1);
        assert_eq!(layer.signature_count(), 1);

        layer.should_allow(sig2);
        assert_eq!(layer.signature_count(), 2);

        // Same signature shouldn't increase count
        layer.should_allow(sig1);
        assert_eq!(layer.signature_count(), 2);
    }

    #[test]
    fn test_metrics_with_eviction() {
        let layer = TracingRateLimitLayer::builder()
            .with_policy(Policy::count_based(1).unwrap())
            .with_max_signatures(3)
            .build()
            .unwrap();

        // Fill up to capacity
        for i in 0..3 {
            let sig = EventSignature::simple("INFO", &format!("test{}", i));
            layer.should_allow(sig);
        }

        assert_eq!(layer.signature_count(), 3);
        assert_eq!(layer.metrics().signatures_evicted(), 0);

        // Add one more, which should trigger eviction
        let sig = EventSignature::simple("INFO", "test3");
        layer.should_allow(sig);

        assert_eq!(layer.signature_count(), 3);
        assert_eq!(layer.metrics().signatures_evicted(), 1);
    }

    #[test]
    fn test_circuit_breaker_observability() {
        use crate::application::circuit_breaker::CircuitState;

        let layer = TracingRateLimitLayer::builder()
            .with_policy(Policy::count_based(2).unwrap())
            .build()
            .unwrap();

        // Check initial circuit breaker state
        let cb = layer.circuit_breaker();
        assert_eq!(cb.state(), CircuitState::Closed);
        assert_eq!(cb.consecutive_failures(), 0);

        // Circuit breaker should remain closed during normal operation
        let sig = EventSignature::simple("INFO", "test");
        layer.should_allow(sig);
        layer.should_allow(sig);
        layer.should_allow(sig);

        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn test_circuit_breaker_fail_open_integration() {
        use crate::application::circuit_breaker::{
            CircuitBreaker, CircuitBreakerConfig, CircuitState,
        };
        use std::time::Duration;

        // Create a circuit breaker with low threshold for testing
        let cb_config = CircuitBreakerConfig {
            failure_threshold: 2,
            recovery_timeout: Duration::from_secs(1),
        };
        let circuit_breaker = Arc::new(CircuitBreaker::with_config(cb_config));

        // Build layer with custom circuit breaker
        let storage = Arc::new(ShardedStorage::new());
        let clock = Arc::new(SystemClock::new());
        let policy = Policy::count_based(2).unwrap();
        let registry = SuppressionRegistry::new(storage, clock, policy);
        let metrics = Metrics::new();
        let limiter = RateLimiter::new(registry, metrics, circuit_breaker.clone());

        let layer = TracingRateLimitLayer {
            limiter,
            _emitter_config: crate::application::emitter::EmitterConfig::new(Duration::from_secs(
                30,
            ))
            .unwrap(),
        };

        let sig = EventSignature::simple("INFO", "test");

        // Normal operation - first 2 events allowed, third suppressed
        assert!(layer.should_allow(sig));
        assert!(layer.should_allow(sig));
        assert!(!layer.should_allow(sig));

        // Circuit should still be closed
        assert_eq!(circuit_breaker.state(), CircuitState::Closed);

        // Manually trigger circuit breaker failures to test fail-open
        circuit_breaker.record_failure();
        circuit_breaker.record_failure();

        // Circuit should now be open
        assert_eq!(circuit_breaker.state(), CircuitState::Open);

        // With circuit open, rate limiter should fail open (allow all events)
        // even though we've already hit the rate limit
        assert!(layer.should_allow(sig));
        assert!(layer.should_allow(sig));
        assert!(layer.should_allow(sig));

        // Metrics should show these as allowed (fail-open behavior)
        let snapshot = layer.metrics().snapshot();
        assert!(snapshot.events_allowed >= 5); // 2 normal + 3 fail-open
    }
}
