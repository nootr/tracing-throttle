//! Tracing integration layer.
//!
//! Provides a `tracing::Layer` implementation that applies rate limiting
//! to log events.

use crate::application::{
    emitter::EmitterConfig,
    limiter::{LimitDecision, RateLimiter},
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
    pub fn build(self) -> TracingRateLimitLayer {
        let clock = self.clock.unwrap_or_else(|| Arc::new(SystemClock::new()));
        let storage = if let Some(max) = self.max_signatures {
            Arc::new(ShardedStorage::with_max_entries(max))
        } else {
            Arc::new(ShardedStorage::new())
        };
        let registry = SuppressionRegistry::new(storage, clock, self.policy);
        let limiter = RateLimiter::new(registry);

        TracingRateLimitLayer {
            limiter,
            _emitter_config: EmitterConfig::new(self.summary_interval),
        }
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
}

impl TracingRateLimitLayer<Arc<ShardedStorage<EventSignature, EventState>>> {
    /// Create a builder for configuring the layer.
    ///
    /// Defaults:
    /// - Policy: count-based (100 events)
    /// - Max signatures: 10,000 (with LRU eviction)
    /// - Summary interval: 30 seconds
    pub fn builder() -> TracingRateLimitLayerBuilder {
        TracingRateLimitLayerBuilder {
            policy: Policy::count_based(100),
            summary_interval: Duration::from_secs(30),
            clock: None,
            max_signatures: Some(10_000),
        }
    }

    /// Create a layer with default settings.
    ///
    /// Equivalent to `TracingRateLimitLayer::builder().build()`.
    ///
    /// Defaults:
    /// - Policy: count-based (100 events)
    /// - Max signatures: 10,000 (with LRU eviction)
    /// - Summary interval: 30 seconds
    pub fn new() -> Self {
        Self::builder().build()
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
            .with_policy(Policy::count_based(50))
            .with_summary_interval(Duration::from_secs(60))
            .build();

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
            .with_policy(Policy::count_based(2))
            .build();

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
            .with_policy(Policy::count_based(3))
            .build();

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
            .with_policy(Policy::count_based(3))
            .build();

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
}
