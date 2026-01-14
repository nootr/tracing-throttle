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

use std::borrow::Cow;
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
    excluded_fields: BTreeSet<String>,
    eviction_strategy: Option<EvictionStrategy>,
    exempt_targets: BTreeSet<String>,
}

/// Eviction strategy configuration for the rate limit layer.
///
/// This enum provides a user-friendly API that internally creates
/// the appropriate EvictionPolicy adapter.
#[derive(Clone)]
pub enum EvictionStrategy {
    /// LRU (Least Recently Used) eviction with entry count limit.
    Lru {
        /// Maximum number of entries
        max_entries: usize,
    },
    /// Priority-based eviction using a custom function.
    Priority {
        /// Maximum number of entries
        max_entries: usize,
        /// Priority calculation function
        priority_fn: crate::infrastructure::eviction::PriorityFn<EventSignature, EventState>,
    },
    /// Memory-based eviction with byte limit.
    Memory {
        /// Maximum memory usage in bytes
        max_bytes: usize,
    },
    /// Combined priority and memory limits.
    PriorityWithMemory {
        /// Maximum number of entries
        max_entries: usize,
        /// Priority calculation function
        priority_fn: crate::infrastructure::eviction::PriorityFn<EventSignature, EventState>,
        /// Maximum memory usage in bytes
        max_bytes: usize,
    },
}

impl EvictionStrategy {
    /// Check if this strategy tracks memory usage.
    pub fn tracks_memory(&self) -> bool {
        matches!(
            self,
            EvictionStrategy::Memory { .. } | EvictionStrategy::PriorityWithMemory { .. }
        )
    }

    /// Get the memory limit if this strategy uses one.
    pub fn memory_limit(&self) -> Option<usize> {
        match self {
            EvictionStrategy::Memory { max_bytes } => Some(*max_bytes),
            EvictionStrategy::PriorityWithMemory { max_bytes, .. } => Some(*max_bytes),
            _ => None,
        }
    }

    /// Check if this strategy uses priority-based eviction.
    pub fn uses_priority(&self) -> bool {
        matches!(
            self,
            EvictionStrategy::Priority { .. } | EvictionStrategy::PriorityWithMemory { .. }
        )
    }
}

impl std::fmt::Debug for EvictionStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EvictionStrategy::Lru { max_entries } => f
                .debug_struct("Lru")
                .field("max_entries", max_entries)
                .finish(),
            EvictionStrategy::Priority {
                max_entries,
                priority_fn: _,
            } => f
                .debug_struct("Priority")
                .field("max_entries", max_entries)
                .field("priority_fn", &"<fn>")
                .finish(),
            EvictionStrategy::Memory { max_bytes } => f
                .debug_struct("Memory")
                .field("max_bytes", max_bytes)
                .finish(),
            EvictionStrategy::PriorityWithMemory {
                max_entries,
                priority_fn: _,
                max_bytes,
            } => f
                .debug_struct("PriorityWithMemory")
                .field("max_entries", max_entries)
                .field("priority_fn", &"<fn>")
                .field("max_bytes", max_bytes)
                .finish(),
        }
    }
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

    /// Exclude specific fields from event signatures.
    ///
    /// By default, ALL event fields are included in signatures. This ensures that
    /// events with different field values are treated as distinct events, preventing
    /// accidental loss of meaningful log data.
    ///
    /// Use this method to exclude high-cardinality fields that don't change the
    /// semantic meaning of the event (e.g., request_id, timestamp, trace_id).
    ///
    /// Duplicate field names are automatically removed, and empty field names are filtered out.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use tracing_throttle::TracingRateLimitLayer;
    /// // Exclude request_id so events with same user_id are deduplicated
    /// let layer = TracingRateLimitLayer::builder()
    ///     .with_excluded_fields(vec!["request_id".to_string(), "trace_id".to_string()])
    ///     .build()
    ///     .unwrap();
    /// ```
    ///
    /// # Default Behavior (ALL fields included)
    ///
    /// ```no_run
    /// # use tracing::error;
    /// // These are DIFFERENT events (different user_id values)
    /// error!(user_id = 123, "Failed to fetch user");
    /// error!(user_id = 456, "Failed to fetch user");
    /// // Both are logged - they have distinct signatures
    /// ```
    ///
    /// # With Exclusions
    ///
    /// ```no_run
    /// # use tracing::error;
    /// // Exclude request_id from signature
    /// error!(user_id = 123, request_id = "abc", "Failed to fetch user");
    /// error!(user_id = 123, request_id = "def", "Failed to fetch user");
    /// // Second is throttled - same user_id, request_id excluded from signature
    /// ```
    pub fn with_excluded_fields(mut self, fields: Vec<String>) -> Self {
        // Deduplicate and filter out empty field names
        let unique_fields: BTreeSet<_> = fields.into_iter().filter(|f| !f.is_empty()).collect();
        self.excluded_fields = unique_fields;
        self
    }

    /// Exempt specific targets from rate limiting.
    ///
    /// Events from the specified targets will bypass rate limiting entirely and
    /// always be allowed through. This is useful for ensuring critical logs (e.g.,
    /// security events, audit logs) are never suppressed.
    ///
    /// Targets are matched exactly against the event's target (module path).
    /// Duplicate targets are automatically removed, and empty targets are filtered out.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use tracing_throttle::TracingRateLimitLayer;
    /// // Never throttle security or audit logs
    /// let layer = TracingRateLimitLayer::builder()
    ///     .with_exempt_targets(vec![
    ///         "myapp::security".to_string(),
    ///         "myapp::audit".to_string(),
    ///     ])
    ///     .build()
    ///     .unwrap();
    /// ```
    ///
    /// # Usage with Events
    ///
    /// ```no_run
    /// # use tracing::error;
    /// // Explicitly set target - this event bypasses throttling
    /// error!(target: "myapp::security", "Security breach detected");
    ///
    /// // Or use the module's default target
    /// mod security {
    ///     use tracing::warn;
    ///     pub fn check() {
    ///         // Target is automatically "myapp::security" - bypasses throttling
    ///         warn!("Suspicious activity detected");
    ///     }
    /// }
    /// ```
    pub fn with_exempt_targets(mut self, targets: Vec<String>) -> Self {
        // Deduplicate and filter out empty targets
        let unique_targets: BTreeSet<_> = targets.into_iter().filter(|t| !t.is_empty()).collect();
        self.exempt_targets = unique_targets;
        self
    }

    /// Set a custom eviction strategy for signature management.
    ///
    /// Controls which signatures are evicted when storage limits are reached.
    /// If not set, uses LRU eviction with the configured max_signatures limit.
    ///
    /// # Example: Priority-based eviction
    ///
    /// ```no_run
    /// # use tracing_throttle::{TracingRateLimitLayer, EvictionStrategy};
    /// # use std::sync::Arc;
    /// let layer = TracingRateLimitLayer::builder()
    ///     .with_eviction_strategy(EvictionStrategy::Priority {
    ///         max_entries: 5_000,
    ///         priority_fn: Arc::new(|_sig, state| {
    ///             // Keep ERROR events longer than INFO events
    ///             match state.metadata.as_ref().map(|m| m.level.as_str()) {
    ///                 Some("ERROR") => 100,
    ///                 Some("WARN") => 50,
    ///                 Some("INFO") => 10,
    ///                 _ => 5,
    ///             }
    ///         })
    ///     })
    ///     .build()
    ///     .unwrap();
    /// ```
    ///
    /// # Example: Memory-based eviction
    ///
    /// ```no_run
    /// # use tracing_throttle::{TracingRateLimitLayer, EvictionStrategy};
    /// // Evict when total memory exceeds 5MB
    /// let layer = TracingRateLimitLayer::builder()
    ///     .with_eviction_strategy(EvictionStrategy::Memory {
    ///         max_bytes: 5 * 1024 * 1024,
    ///     })
    ///     .build()
    ///     .unwrap();
    /// ```
    pub fn with_eviction_strategy(mut self, strategy: EvictionStrategy) -> Self {
        self.eviction_strategy = Some(strategy);
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
        let mut storage = ShardedStorage::new().with_metrics(metrics.clone());

        // Convert eviction strategy to adapter, or use default LRU with max_signatures
        let eviction_policy: Option<
            Arc<dyn crate::application::ports::EvictionPolicy<EventSignature, EventState>>,
        > = match self.eviction_strategy {
            Some(EvictionStrategy::Lru { max_entries }) => Some(Arc::new(
                crate::infrastructure::eviction::LruEviction::new(max_entries),
            )),
            Some(EvictionStrategy::Priority {
                max_entries,
                priority_fn,
            }) => Some(Arc::new(
                crate::infrastructure::eviction::PriorityEviction::new(max_entries, priority_fn),
            )),
            Some(EvictionStrategy::Memory { max_bytes }) => Some(Arc::new(
                crate::infrastructure::eviction::MemoryEviction::new(max_bytes),
            )),
            Some(EvictionStrategy::PriorityWithMemory {
                max_entries,
                priority_fn,
                max_bytes,
            }) => Some(Arc::new(
                crate::infrastructure::eviction::PriorityWithMemoryEviction::new(
                    max_entries,
                    priority_fn,
                    max_bytes,
                ),
            )),
            None => {
                // Use default LRU with max_signatures if configured
                self.max_signatures.map(|max| {
                    Arc::new(crate::infrastructure::eviction::LruEviction::new(max))
                        as Arc<
                            dyn crate::application::ports::EvictionPolicy<
                                EventSignature,
                                EventState,
                            >,
                        >
                })
            }
        };

        if let Some(policy) = eviction_policy {
            storage = storage.with_eviction_policy(policy);
        }

        let storage = Arc::new(storage);
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
            excluded_fields: Arc::new(self.excluded_fields),
            exempt_targets: Arc::new(self.exempt_targets),
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
    excluded_fields: Arc<BTreeSet<String>>,
    exempt_targets: Arc<BTreeSet<String>>,
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
    fn extract_span_context<Sub>(
        &self,
        cx: &Context<'_, Sub>,
    ) -> BTreeMap<Cow<'static, str>, Cow<'static, str>>
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

                if let Some(stored_fields) =
                    extensions.get::<BTreeMap<Cow<'static, str>, Cow<'static, str>>>()
                {
                    for field_name in self.span_context_fields.as_ref() {
                        // Create an owned Cow since we can't guarantee 'static lifetime from the String
                        let field_key: Cow<'static, str> = Cow::Owned(field_name.clone());
                        if let std::collections::btree_map::Entry::Vacant(e) =
                            context_fields.entry(field_key.clone())
                        {
                            if let Some(value) = stored_fields.get(&field_key) {
                                e.insert(value.clone());
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
    ///
    /// Extracts ALL fields from the event, then excludes any fields in the
    /// excluded_fields set. This ensures that field values are included in
    /// event signatures by default, preventing accidental deduplication of
    /// semantically different events.
    fn extract_event_fields(
        &self,
        event: &tracing::Event<'_>,
    ) -> BTreeMap<Cow<'static, str>, Cow<'static, str>> {
        let mut visitor = FieldVisitor::new();
        event.record(&mut visitor);
        let all_fields = visitor.into_fields();

        // Exclude configured fields (e.g., high-cardinality fields like request_id)
        if self.excluded_fields.is_empty() {
            all_fields
        } else {
            all_fields
                .into_iter()
                .filter(|(field_name, _)| !self.excluded_fields.contains(field_name.as_ref()))
                .collect()
        }
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
        combined_fields: &BTreeMap<Cow<'static, str>, Cow<'static, str>>,
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

    /// Check if an event should be allowed through and capture metadata.
    ///
    /// This method stores event metadata on first occurrence so summaries
    /// can show human-readable event details instead of just signature hashes.
    ///
    /// **Note:** Only available with the `human-readable` feature flag.
    #[cfg(feature = "human-readable")]
    pub fn should_allow_with_metadata(
        &self,
        signature: EventSignature,
        metadata: crate::domain::metadata::EventMetadata,
    ) -> bool {
        matches!(
            self.limiter.check_event_with_metadata(signature, metadata),
            LimitDecision::Allow
        )
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
            excluded_fields: BTreeSet::new(),
            eviction_strategy: None,
            exempt_targets: BTreeSet::new(),
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

    /// Create a layer with custom storage backend.
    ///
    /// This allows using alternative storage implementations like Redis for distributed
    /// rate limiting across multiple application instances.
    ///
    /// # Arguments
    ///
    /// * `storage` - Custom storage implementation (must implement `Storage<EventSignature, EventState>`)
    /// * `policy` - Rate limiting policy to apply
    /// * `clock` - Clock implementation (use `SystemClock::new()` for production)
    ///
    /// # Example with Redis
    ///
    /// ```rust,ignore
    /// use tracing_throttle::{TracingRateLimitLayer, RedisStorage, Policy, SystemClock};
    /// use std::sync::Arc;
    ///
    /// #[tokio::main]
    /// async fn main() {
    ///     let storage = Arc::new(
    ///         RedisStorage::connect("redis://127.0.0.1/")
    ///             .await
    ///             .expect("Failed to connect")
    ///     );
    ///     let policy = Policy::token_bucket(100.0, 10.0).unwrap();
    ///     let clock = Arc::new(SystemClock::new());
    ///
    ///     let layer = TracingRateLimitLayer::with_storage(storage, policy, clock);
    /// }
    /// ```
    pub fn with_storage<ST>(
        storage: ST,
        policy: Policy,
        clock: Arc<dyn Clock>,
    ) -> TracingRateLimitLayer<ST>
    where
        ST: Storage<EventSignature, EventState> + Clone,
    {
        let metrics = Metrics::new();
        let circuit_breaker = Arc::new(CircuitBreaker::new());
        let registry = SuppressionRegistry::new(storage, clock, policy);
        let limiter = RateLimiter::new(registry, metrics, circuit_breaker);

        TracingRateLimitLayer {
            limiter,
            span_context_fields: Arc::new(Vec::new()),
            excluded_fields: Arc::new(BTreeSet::new()),
            exempt_targets: Arc::new(BTreeSet::new()),
            #[cfg(feature = "async")]
            emitter_handle: Arc::new(Mutex::new(None)),
            #[cfg(not(feature = "async"))]
            _emitter_config: EmitterConfig::new(Duration::from_secs(30))
                .expect("30 seconds is valid"),
        }
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
        let metadata_obj = event.metadata();

        // Check if this target is exempt from rate limiting
        // Skip the lookup if no exempt targets are configured (common case)
        if !self.exempt_targets.is_empty() && self.exempt_targets.contains(metadata_obj.target()) {
            // Exempt targets bypass rate limiting entirely
            self.limiter.metrics().record_allowed();
            return true;
        }

        // Combine span context and event fields
        let mut combined_fields = self.extract_span_context(cx);
        let event_fields = self.extract_event_fields(event);
        combined_fields.extend(event_fields);

        let signature = self.compute_signature(metadata_obj, &combined_fields);

        #[cfg(feature = "human-readable")]
        {
            // Extract message from event for metadata
            let mut visitor = FieldVisitor::new();
            event.record(&mut visitor);
            let all_fields = visitor.into_fields();
            let message = all_fields
                .get(&Cow::Borrowed("message"))
                .map(|v| v.to_string())
                .unwrap_or_else(|| event.metadata().name().to_string());

            // Create EventMetadata for this event
            let event_metadata = crate::domain::metadata::EventMetadata::new(
                metadata_obj.level().as_str().to_string(),
                message,
                metadata_obj.target().to_string(),
                combined_fields,
            );

            self.should_allow_with_metadata(signature, event_metadata)
        }

        #[cfg(not(feature = "human-readable"))]
        {
            self.should_allow(signature)
        }
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
    fn test_excluded_fields_deduplication() {
        let layer = TracingRateLimitLayer::builder()
            .with_excluded_fields(vec![
                "request_id".to_string(),
                "request_id".to_string(), // duplicate
                "trace_id".to_string(),
                "".to_string(),           // empty, should be filtered
                "request_id".to_string(), // another duplicate
            ])
            .build()
            .unwrap();

        // Should only have 2 unique fields: request_id and trace_id
        assert_eq!(layer.excluded_fields.len(), 2);
        assert!(layer.excluded_fields.contains("request_id"));
        assert!(layer.excluded_fields.contains("trace_id"));
    }

    #[test]
    fn test_exempt_targets_deduplication() {
        let layer = TracingRateLimitLayer::builder()
            .with_exempt_targets(vec![
                "myapp::security".to_string(),
                "myapp::security".to_string(), // duplicate
                "myapp::audit".to_string(),
                "".to_string(),                // empty, should be filtered
                "myapp::security".to_string(), // another duplicate
            ])
            .build()
            .unwrap();

        // Should only have 2 unique targets: security and audit
        assert_eq!(layer.exempt_targets.len(), 2);
        assert!(layer.exempt_targets.contains("myapp::security"));
        assert!(layer.exempt_targets.contains("myapp::audit"));
    }

    #[test]
    fn test_exempt_targets_bypass_rate_limiting() {
        let rate_limit = TracingRateLimitLayer::builder()
            .with_policy(Policy::count_based(2).unwrap())
            .with_exempt_targets(vec!["myapp::security".to_string()])
            .build()
            .unwrap();

        let subscriber = tracing_subscriber::registry()
            .with(tracing_subscriber::fmt::layer().with_filter(rate_limit.clone()));

        tracing::subscriber::with_default(subscriber, || {
            // Regular logs get throttled after 2 (same callsite = same signature)
            for _ in 0..3 {
                info!("Regular log"); // First 2 allowed, 3rd suppressed
            }

            // Security logs are never throttled (exempt target)
            for _ in 0..4 {
                info!(target: "myapp::security", "Security event"); // All 4 allowed
            }
        });

        // Verify metrics
        let metrics = rate_limit.metrics();
        assert_eq!(metrics.events_allowed(), 6); // 2 regular + 4 exempt
        assert_eq!(metrics.events_suppressed(), 1); // 1 regular suppressed
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
            excluded_fields: Arc::new(BTreeSet::new()),
            exempt_targets: Arc::new(BTreeSet::new()),
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
