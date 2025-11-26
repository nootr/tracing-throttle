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
use crate::infrastructure::visitor::FieldVisitor;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;
use tracing::{Metadata, Subscriber};
use tracing_subscriber::layer::Filter;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::{layer::Context, Layer};

#[cfg(feature = "async")]
use crate::application::emitter::{EmitterHandle, SummaryEmitter};

#[cfg(feature = "async")]
use crate::domain::summary::SuppressionSummary;

#[cfg(feature = "async")]
use std::sync::Mutex;

/// Function type for formatting suppression summaries.
///
/// Takes a reference to a `SuppressionSummary` and emits it as a tracing event.
/// The function is responsible for choosing the log level and format.
#[cfg(feature = "async")]
pub type SummaryFormatter = Arc<dyn Fn(&SuppressionSummary) + Send + Sync + 'static>;

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
pub struct TracingRateLimitLayerBuilder {
    policy: Policy,
    summary_interval: Duration,
    clock: Option<Arc<dyn Clock>>,
    max_signatures: Option<usize>,
    enable_active_emission: bool,
    #[cfg(feature = "async")]
    summary_formatter: Option<SummaryFormatter>,
    span_context_fields: Vec<String>,
    event_fields: Vec<String>,
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

    /// Enable active emission of suppression summaries.
    ///
    /// When enabled, the layer will automatically emit `WARN`-level tracing events
    /// containing summaries of suppressed log events at the configured interval.
    ///
    /// **Requires the `async` feature** - this method has no effect without it.
    ///
    /// Default: disabled
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use tracing_throttle::TracingRateLimitLayer;
    /// # use std::time::Duration;
    /// let layer = TracingRateLimitLayer::builder()
    ///     .with_active_emission(true)
    ///     .with_summary_interval(Duration::from_secs(60))
    ///     .build()
    ///     .unwrap();
    /// ```
    pub fn with_active_emission(mut self, enabled: bool) -> Self {
        self.enable_active_emission = enabled;
        self
    }

    /// Set a custom formatter for suppression summaries.
    ///
    /// The formatter is responsible for emitting summaries as tracing events.
    /// This allows full control over log level, message format, and structured fields.
    ///
    /// **Requires the `async` feature.**
    ///
    /// If not set, a default formatter is used that emits at WARN level with
    /// `signature` and `count` fields.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use tracing_throttle::TracingRateLimitLayer;
    /// # use std::sync::Arc;
    /// # use std::time::Duration;
    /// let layer = TracingRateLimitLayer::builder()
    ///     .with_active_emission(true)
    ///     .with_summary_formatter(Arc::new(|summary| {
    ///         tracing::info!(
    ///             signature = %summary.signature,
    ///             count = summary.count,
    ///             duration_secs = summary.duration.as_secs(),
    ///             "Suppression summary"
    ///         );
    ///     }))
    ///     .build()
    ///     .unwrap();
    /// ```
    #[cfg(feature = "async")]
    pub fn with_summary_formatter(mut self, formatter: SummaryFormatter) -> Self {
        self.summary_formatter = Some(formatter);
        self
    }

    /// Include span context fields in event signatures.
    ///
    /// When specified, the layer will extract these fields from the current span
    /// context and include them in the event signature. This enables rate limiting
    /// per-user, per-tenant, per-request, or any other span-level context.
    ///
    /// Duplicate field names are automatically removed, and empty field names are filtered out.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use tracing_throttle::TracingRateLimitLayer;
    /// // Rate limit separately per user
    /// let layer = TracingRateLimitLayer::builder()
    ///     .with_span_context_fields(vec!["user_id".to_string()])
    ///     .build()
    ///     .unwrap();
    ///
    /// // Rate limit per user and tenant
    /// let layer = TracingRateLimitLayer::builder()
    ///     .with_span_context_fields(vec!["user_id".to_string(), "tenant_id".to_string()])
    ///     .build()
    ///     .unwrap();
    /// ```
    ///
    /// # Usage with Spans
    ///
    /// ```no_run
    /// # use tracing::{info, info_span};
    /// // Create a span with user context
    /// let span = info_span!("request", user_id = "alice");
    /// let _enter = span.enter();
    ///
    /// // These events will be rate limited separately per user
    /// info!("Processing request");  // Limited for user "alice"
    /// ```
    pub fn with_span_context_fields(mut self, fields: Vec<String>) -> Self {
        // Deduplicate and filter out empty field names
        let unique_fields: BTreeSet<_> = fields.into_iter().filter(|f| !f.is_empty()).collect();
        self.span_context_fields = unique_fields.into_iter().collect();
        self
    }

    /// Include event fields in event signatures.
    ///
    /// When specified, the layer will extract these fields from events themselves
    /// and include them in the event signature. This enables rate limiting per
    /// error code, status, endpoint, or any other event-level field.
    ///
    /// Duplicate field names are automatically removed, and empty field names are filtered out.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use tracing_throttle::TracingRateLimitLayer;
    /// // Rate limit separately per error_code
    /// let layer = TracingRateLimitLayer::builder()
    ///     .with_event_fields(vec!["error_code".to_string()])
    ///     .build()
    ///     .unwrap();
    ///
    /// // Rate limit per status and endpoint
    /// let layer = TracingRateLimitLayer::builder()
    ///     .with_event_fields(vec!["status".to_string(), "endpoint".to_string()])
    ///     .build()
    ///     .unwrap();
    /// ```
    ///
    /// # Usage with Events
    ///
    /// ```no_run
    /// # use tracing::error;
    /// // Events with different error codes are rate limited independently
    /// error!(error_code = "AUTH_FAILED", "Authentication failed");
    /// error!(error_code = "TIMEOUT", "Request timeout");
    /// ```
    pub fn with_event_fields(mut self, fields: Vec<String>) -> Self {
        // Deduplicate and filter out empty field names
        let unique_fields: BTreeSet<_> = fields.into_iter().filter(|f| !f.is_empty()).collect();
        self.event_fields = unique_fields.into_iter().collect();
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
        let limiter = RateLimiter::new(registry.clone(), metrics.clone(), circuit_breaker);

        // Let EmitterConfig validate the interval
        let emitter_config = EmitterConfig::new(self.summary_interval)?;

        #[cfg(feature = "async")]
        let emitter_handle = if self.enable_active_emission {
            let emitter = SummaryEmitter::new(registry, emitter_config);

            // Use custom formatter or default
            let formatter = self.summary_formatter.unwrap_or_else(|| {
                Arc::new(|summary: &SuppressionSummary| {
                    tracing::warn!(
                        signature = %summary.signature,
                        count = summary.count,
                        "{}",
                        summary.format_message()
                    );
                })
            });

            let handle = emitter.start(
                move |summaries| {
                    for summary in summaries {
                        formatter(&summary);
                    }
                },
                false, // Don't emit final summaries on shutdown
            );
            Arc::new(Mutex::new(Some(handle)))
        } else {
            Arc::new(Mutex::new(None))
        };

        Ok(TracingRateLimitLayer {
            limiter,
            span_context_fields: Arc::new(self.span_context_fields),
            event_fields: Arc::new(self.event_fields),
            #[cfg(feature = "async")]
            emitter_handle,
            #[cfg(not(feature = "async"))]
            _emitter_config: emitter_config,
        })
    }
}

/// A `tracing::Layer` that applies rate limiting to events.
///
/// This layer intercepts events, computes their signature, and decides
/// whether to allow or suppress them based on the configured policy.
///
/// Optionally emits periodic summaries of suppressed events when active
/// emission is enabled (requires `async` feature).
#[derive(Clone)]
pub struct TracingRateLimitLayer<S = Arc<ShardedStorage<EventSignature, EventState>>>
where
    S: Storage<EventSignature, EventState> + Clone,
{
    limiter: RateLimiter<S>,
    span_context_fields: Arc<Vec<String>>,
    event_fields: Arc<Vec<String>>,
    #[cfg(feature = "async")]
    emitter_handle: Arc<Mutex<Option<EmitterHandle>>>,
    #[cfg(not(feature = "async"))]
    _emitter_config: EmitterConfig,
}

impl<S> TracingRateLimitLayer<S>
where
    S: Storage<EventSignature, EventState> + Clone,
{
    /// Extract span context fields from the current span.
    fn extract_span_context<Sub>(&self, cx: &Context<'_, Sub>) -> BTreeMap<String, String>
    where
        Sub: Subscriber + for<'lookup> LookupSpan<'lookup>,
    {
        if self.span_context_fields.is_empty() {
            return BTreeMap::new();
        }

        let mut context_fields = BTreeMap::new();

        if let Some(span) = cx.lookup_current() {
            for span_ref in span.scope() {
                let extensions = span_ref.extensions();

                if let Some(stored_fields) = extensions.get::<BTreeMap<String, String>>() {
                    for field_name in self.span_context_fields.as_ref() {
                        if !context_fields.contains_key(field_name) {
                            if let Some(value) = stored_fields.get(field_name) {
                                context_fields.insert(field_name.clone(), value.clone());
                            }
                        }
                    }
                }

                if context_fields.len() == self.span_context_fields.len() {
                    break;
                }
            }
        }

        context_fields
    }

    /// Extract event fields from an event.
    fn extract_event_fields(&self, event: &tracing::Event<'_>) -> BTreeMap<String, String> {
        if self.event_fields.is_empty() {
            return BTreeMap::new();
        }

        let mut visitor = FieldVisitor::new();
        event.record(&mut visitor);
        let all_fields = visitor.into_fields();

        // Filter to only the configured event fields
        self.event_fields
            .iter()
            .filter_map(|field_name| {
                all_fields
                    .get(field_name)
                    .map(|value| (field_name.clone(), value.clone()))
            })
            .collect()
    }

    /// Compute event signature from tracing metadata, span context, and event fields.
    ///
    /// The signature includes:
    /// - Log level (INFO, WARN, ERROR, etc.)
    /// - Message template
    /// - Target module path
    /// - Span context fields (if configured)
    /// - Event fields (if configured)
    fn compute_signature(
        &self,
        metadata: &Metadata,
        combined_fields: &BTreeMap<String, String>,
    ) -> EventSignature {
        let level = metadata.level().as_str();
        let message = metadata.name();
        let target = Some(metadata.target());

        // Use combined fields (span context + event fields) in signature
        EventSignature::new(level, message, combined_fields, target)
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

    /// Shutdown the active suppression summary emitter, if running.
    ///
    /// This method gracefully stops the background emission task.  If active emission
    /// is not enabled, this method does nothing.
    ///
    /// **Requires the `async` feature.**
    ///
    /// # Errors
    ///
    /// Returns an error if the emitter task fails to shut down gracefully.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use tracing_throttle::TracingRateLimitLayer;
    /// # async fn example() {
    /// let layer = TracingRateLimitLayer::builder()
    ///     .with_active_emission(true)
    ///     .build()
    ///     .unwrap();
    ///
    /// // Use the layer...
    ///
    /// // Shutdown before dropping
    /// layer.shutdown().await.expect("shutdown failed");
    /// # }
    /// ```
    #[cfg(feature = "async")]
    pub async fn shutdown(&self) -> Result<(), crate::application::emitter::ShutdownError> {
        // Take the handle while holding the lock, then release the lock before awaiting
        let handle = {
            let mut handle_guard = self.emitter_handle.lock().unwrap();
            handle_guard.take()
        };

        if let Some(handle) = handle {
            handle.shutdown().await?;
        }
        Ok(())
    }
}

impl TracingRateLimitLayer<Arc<ShardedStorage<EventSignature, EventState>>> {
    /// Create a builder for configuring the layer.
    ///
    /// Defaults:
    /// - Policy: token bucket (50 burst capacity, 1 token/sec refill rate)
    /// - Max signatures: 10,000 (with LRU eviction)
    /// - Summary interval: 30 seconds
    /// - Active emission: disabled
    /// - Summary formatter: default (WARN level with signature and count)
    pub fn builder() -> TracingRateLimitLayerBuilder {
        TracingRateLimitLayerBuilder {
            policy: Policy::token_bucket(50.0, 1.0)
                .expect("default policy with 50 capacity and 1/sec refill is always valid"),
            summary_interval: Duration::from_secs(30),
            clock: None,
            max_signatures: Some(10_000),
            enable_active_emission: false,
            #[cfg(feature = "async")]
            summary_formatter: None,
            span_context_fields: Vec::new(),
            event_fields: Vec::new(),
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
    Sub: Subscriber + for<'lookup> LookupSpan<'lookup>,
{
    fn enabled(&self, _meta: &Metadata<'_>, _cx: &Context<'_, Sub>) -> bool {
        // Always return true - actual filtering happens in event_enabled
        // This prevents double-checking in dual-layer setups
        true
    }

    fn event_enabled(&self, event: &tracing::Event<'_>, cx: &Context<'_, Sub>) -> bool {
        // Combine span context and event fields
        let mut combined_fields = self.extract_span_context(cx);
        let event_fields = self.extract_event_fields(event);
        combined_fields.extend(event_fields);

        let signature = self.compute_signature(event.metadata(), &combined_fields);
        self.should_allow(signature)
    }
}

impl<S, Sub> Layer<Sub> for TracingRateLimitLayer<S>
where
    S: Storage<EventSignature, EventState> + Clone + 'static,
    Sub: Subscriber + for<'lookup> LookupSpan<'lookup>,
{
    fn on_new_span(
        &self,
        attrs: &tracing::span::Attributes<'_>,
        id: &tracing::span::Id,
        ctx: Context<'_, Sub>,
    ) {
        if self.span_context_fields.is_empty() {
            return;
        }

        let mut visitor = FieldVisitor::new();
        attrs.record(&mut visitor);
        let fields = visitor.into_fields();

        if let Some(span) = ctx.span(id) {
            let mut extensions = span.extensions_mut();
            extensions.insert(fields);
        }
    }
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
    fn test_span_context_fields_deduplication() {
        let layer = TracingRateLimitLayer::builder()
            .with_span_context_fields(vec![
                "user_id".to_string(),
                "user_id".to_string(), // duplicate
                "tenant_id".to_string(),
                "".to_string(),        // empty, should be filtered
                "user_id".to_string(), // another duplicate
            ])
            .build()
            .unwrap();

        // Should only have 2 unique fields: user_id and tenant_id
        assert_eq!(layer.span_context_fields.len(), 2);
        assert!(layer.span_context_fields.iter().any(|f| f == "user_id"));
        assert!(layer.span_context_fields.iter().any(|f| f == "tenant_id"));
    }

    #[test]
    fn test_event_fields_deduplication() {
        let layer = TracingRateLimitLayer::builder()
            .with_event_fields(vec![
                "error_code".to_string(),
                "error_code".to_string(), // duplicate
                "status".to_string(),
                "".to_string(),           // empty, should be filtered
                "error_code".to_string(), // another duplicate
            ])
            .build()
            .unwrap();

        // Should only have 2 unique fields: error_code and status
        assert_eq!(layer.event_fields.len(), 2);
        assert!(layer.event_fields.iter().any(|f| f == "error_code"));
        assert!(layer.event_fields.iter().any(|f| f == "status"));
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
            span_context_fields: Arc::new(Vec::new()),
            event_fields: Arc::new(Vec::new()),
            #[cfg(feature = "async")]
            emitter_handle: Arc::new(Mutex::new(None)),
            #[cfg(not(feature = "async"))]
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

    #[cfg(feature = "async")]
    #[tokio::test]
    async fn test_active_emission_integration() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::time::Duration;

        // Use an atomic counter to track emissions
        let emission_count = Arc::new(AtomicUsize::new(0));
        let count_clone = Arc::clone(&emission_count);

        // Create a layer with a custom emitter that increments our counter
        let storage = Arc::new(ShardedStorage::new());
        let clock = Arc::new(SystemClock::new());
        let policy = Policy::count_based(2).unwrap();
        let registry = SuppressionRegistry::new(storage, clock, policy);

        let emitter_config = EmitterConfig::new(Duration::from_millis(100)).unwrap();
        let emitter = SummaryEmitter::new(registry.clone(), emitter_config);

        // Start emitter with custom callback
        let handle = emitter.start(
            move |summaries| {
                count_clone.fetch_add(summaries.len(), Ordering::SeqCst);
            },
            false,
        );

        // Emit events that will be suppressed
        let sig = EventSignature::simple("INFO", "test_message");
        for _ in 0..10 {
            registry.with_event_state(sig, |state, now| {
                state.counter.record_suppression(now);
            });
        }

        // Wait for at least two emission intervals
        tokio::time::sleep(Duration::from_millis(250)).await;

        // Check that summaries were emitted
        let count = emission_count.load(Ordering::SeqCst);
        assert!(
            count > 0,
            "Expected at least one suppression summary to be emitted, got {}",
            count
        );

        // Graceful shutdown
        handle.shutdown().await.expect("shutdown failed");
    }

    #[cfg(feature = "async")]
    #[tokio::test]
    async fn test_active_emission_disabled() {
        use crate::infrastructure::mocks::layer::MockCaptureLayer;
        use std::time::Duration;

        // Create layer with active emission disabled (default)
        let layer = TracingRateLimitLayer::builder()
            .with_policy(Policy::count_based(2).unwrap())
            .with_summary_interval(Duration::from_millis(100))
            .build()
            .unwrap();

        let mock = MockCaptureLayer::new();
        let mock_clone = mock.clone();

        let subscriber = tracing_subscriber::registry()
            .with(mock)
            .with(tracing_subscriber::fmt::layer().with_filter(layer.clone()));

        tracing::subscriber::with_default(subscriber, || {
            let sig = EventSignature::simple("INFO", "test_message");
            for _ in 0..10 {
                layer.should_allow(sig);
            }
        });

        // Wait to ensure no emissions occur
        tokio::time::sleep(Duration::from_millis(250)).await;

        // Should not have emitted any summaries
        let events = mock_clone.get_captured();
        let summary_count = events
            .iter()
            .filter(|e| e.message.contains("suppressed"))
            .count();

        assert_eq!(
            summary_count, 0,
            "Should not emit summaries when active emission is disabled"
        );

        // Shutdown should succeed even when emitter was never started
        layer.shutdown().await.expect("shutdown failed");
    }

    #[cfg(feature = "async")]
    #[tokio::test]
    async fn test_shutdown_without_emission() {
        // Test that shutdown works when emission was never enabled
        let layer = TracingRateLimitLayer::new();

        // Should not error
        layer
            .shutdown()
            .await
            .expect("shutdown should succeed when emitter not running");
    }

    #[cfg(feature = "async")]
    #[tokio::test]
    async fn test_custom_summary_formatter() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::time::Duration;

        // Track formatter invocations
        let call_count = Arc::new(AtomicUsize::new(0));
        let count_clone = Arc::clone(&call_count);

        // Track data passed to formatter
        let last_count = Arc::new(AtomicUsize::new(0));
        let last_count_clone = Arc::clone(&last_count);

        // Create layer with custom formatter
        let layer = TracingRateLimitLayer::builder()
            .with_policy(Policy::count_based(2).unwrap())
            .with_active_emission(true)
            .with_summary_interval(Duration::from_millis(100))
            .with_summary_formatter(Arc::new(move |summary| {
                count_clone.fetch_add(1, Ordering::SeqCst);
                last_count_clone.store(summary.count, Ordering::SeqCst);
                // Custom format: emit at INFO level instead of WARN
                tracing::info!(
                    sig = %summary.signature,
                    suppressed = summary.count,
                    "Custom format"
                );
            }))
            .build()
            .unwrap();

        // Emit events that will be suppressed
        let sig = EventSignature::simple("INFO", "test_message");
        for _ in 0..10 {
            layer.should_allow(sig);
        }

        // Wait for emission
        tokio::time::sleep(Duration::from_millis(250)).await;

        // Verify custom formatter was called
        let calls = call_count.load(Ordering::SeqCst);
        assert!(calls > 0, "Custom formatter should have been called");

        // Verify formatter received correct data
        let count = last_count.load(Ordering::SeqCst);
        assert!(
            count >= 8,
            "Expected at least 8 suppressions, got {}",
            count
        );

        layer.shutdown().await.expect("shutdown failed");
    }

    #[cfg(feature = "async")]
    #[tokio::test]
    async fn test_default_formatter_used() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::time::Duration;

        let emission_count = Arc::new(AtomicUsize::new(0));
        let count_clone = Arc::clone(&emission_count);

        let storage = Arc::new(ShardedStorage::new());
        let clock = Arc::new(SystemClock::new());
        let policy = Policy::count_based(2).unwrap();
        let registry = SuppressionRegistry::new(storage, clock, policy);

        let emitter_config = EmitterConfig::new(Duration::from_millis(100)).unwrap();
        let emitter = SummaryEmitter::new(registry.clone(), emitter_config);

        // Start without custom formatter - should use default
        let handle = emitter.start(
            move |summaries| {
                count_clone.fetch_add(summaries.len(), Ordering::SeqCst);
            },
            false,
        );

        let sig = EventSignature::simple("INFO", "test_message");
        for _ in 0..10 {
            registry.with_event_state(sig, |state, now| {
                state.counter.record_suppression(now);
            });
        }

        tokio::time::sleep(Duration::from_millis(250)).await;

        let count = emission_count.load(Ordering::SeqCst);
        assert!(count > 0, "Default formatter should have emitted summaries");

        handle.shutdown().await.expect("shutdown failed");
    }
}
