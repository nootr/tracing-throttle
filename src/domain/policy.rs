//! Rate limiting policies for event suppression.
//!
//! This module defines the core trait for rate limiting policies and provides
//! several built-in implementations.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

#[cfg(feature = "redis-storage")]
use serde::{Deserialize, Serialize};

/// Error returned when policy validation fails.
///
/// This error type represents domain-level validation rules for rate limiting
/// policies. The domain defines what constitutes a valid policy configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyError {
    /// Maximum count must be greater than zero
    ZeroMaxCount,
    /// Maximum events must be greater than zero
    ZeroMaxEvents,
    /// Time window duration must be greater than zero
    ZeroWindowDuration,
    /// Bucket capacity must be greater than zero
    ZeroCapacity,
    /// Refill rate must be greater than zero
    ZeroRefillRate,
}

impl std::fmt::Display for PolicyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PolicyError::ZeroMaxCount => write!(f, "max_count must be greater than 0"),
            PolicyError::ZeroMaxEvents => write!(f, "max_events must be greater than 0"),
            PolicyError::ZeroWindowDuration => write!(f, "window duration must be greater than 0"),
            PolicyError::ZeroCapacity => write!(f, "capacity must be greater than 0"),
            PolicyError::ZeroRefillRate => write!(f, "refill_rate must be greater than 0"),
        }
    }
}

impl std::error::Error for PolicyError {}

/// Decision made by a rate limiting policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyDecision {
    /// Allow the event to be emitted
    Allow,
    /// Suppress the event (don't emit it)
    Suppress,
}

/// Trait for implementing rate limiting policies.
///
/// Policies determine whether an event should be allowed or suppressed based
/// on historical event patterns.
pub trait RateLimitPolicy: Send + Sync {
    /// Register a new event occurrence and decide whether to allow or suppress it.
    ///
    /// # Arguments
    /// * `timestamp` - When the event occurred
    ///
    /// # Returns
    /// A `PolicyDecision` indicating whether to allow or suppress the event.
    fn register_event(&mut self, timestamp: Instant) -> PolicyDecision;

    /// Reset the policy state.
    ///
    /// Called when starting a new tracking period or when clearing history.
    fn reset(&mut self);
}

/// Count-based rate limiting policy.
///
/// Allows up to N events, then suppresses all subsequent events.
///
/// # Example
/// ```
/// use tracing_throttle::{CountBasedPolicy, RateLimitPolicy};
/// use std::time::Instant;
///
/// let mut policy = CountBasedPolicy::new(3).unwrap();
/// let now = Instant::now();
///
/// // First 3 events allowed
/// assert!(policy.register_event(now).is_allow());
/// assert!(policy.register_event(now).is_allow());
/// assert!(policy.register_event(now).is_allow());
///
/// // 4th and beyond suppressed
/// assert!(policy.register_event(now).is_suppress());
/// assert!(policy.register_event(now).is_suppress());
/// ```
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "redis-storage", derive(Serialize, Deserialize))]
pub struct CountBasedPolicy {
    max_count: usize,
    current_count: usize,
}

impl CountBasedPolicy {
    /// Create a new count-based policy.
    ///
    /// # Arguments
    /// * `max_count` - Maximum number of events to allow before suppressing (must be > 0)
    ///
    /// # Errors
    /// Returns `PolicyError::ZeroMaxCount` if `max_count` is 0.
    pub fn new(max_count: usize) -> Result<Self, PolicyError> {
        if max_count == 0 {
            return Err(PolicyError::ZeroMaxCount);
        }
        Ok(Self {
            max_count,
            current_count: 0,
        })
    }
}

impl RateLimitPolicy for CountBasedPolicy {
    fn register_event(&mut self, _timestamp: Instant) -> PolicyDecision {
        self.current_count += 1;
        if self.current_count <= self.max_count {
            PolicyDecision::Allow
        } else {
            PolicyDecision::Suppress
        }
    }

    fn reset(&mut self) {
        self.current_count = 0;
    }
}

/// Time-window rate limiting policy.
///
/// Allows up to K events within a sliding time window. Events outside the
/// window are automatically expired.
///
/// # Example
/// ```
/// use tracing_throttle::{TimeWindowPolicy, RateLimitPolicy};
/// use std::time::{Duration, Instant};
///
/// let mut policy = TimeWindowPolicy::new(2, Duration::from_secs(60)).unwrap();
/// let now = Instant::now();
///
/// // First 2 events allowed
/// assert!(policy.register_event(now).is_allow());
/// assert!(policy.register_event(now).is_allow());
///
/// // 3rd event suppressed (within window)
/// assert!(policy.register_event(now).is_suppress());
///
/// // After window expires, events are allowed again
/// let after_window = now + Duration::from_secs(61);
/// assert!(policy.register_event(after_window).is_allow());
/// assert!(policy.register_event(after_window).is_allow());
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct TimeWindowPolicy {
    max_events: usize,
    window_duration: Duration,
    event_timestamps: VecDeque<Instant>,
}

#[cfg(feature = "redis-storage")]
impl Serialize for TimeWindowPolicy {
    /// Serialize TimeWindowPolicy for Redis storage.
    ///
    /// # Serialization Strategy
    ///
    /// Event timestamps (Instant) are serialized as relative offsets from the first
    /// timestamp in nanoseconds. This approach is chosen because:
    ///
    /// 1. Instant is not serializable (system-dependent, no epoch)
    /// 2. We only care about relative timing between events, not absolute times
    /// 3. Reduces serialized size (offsets vs full timestamps)
    ///
    /// # Important Note on Deserialization
    ///
    /// When deserializing, timestamps are reconstructed relative to the current time
    /// (Instant::now()). This means the time window effectively "resets" when loaded
    /// from Redis. This is acceptable because:
    ///
    /// - The relative spacing between events is preserved
    /// - Old events will naturally expire based on window_duration
    /// - This prevents issues with long-running processes where Instant could overflow
    ///
    /// **Trade-off**: Events near window expiration may get extra lifetime after reload,
    /// but this is bounded by the window duration and considered acceptable for the
    /// distributed rate limiting use case.
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;

        // Convert Instants to nanoseconds relative to the first timestamp
        let base = self.event_timestamps.front().copied();
        let timestamps_nanos: Vec<u64> = if let Some(base_instant) = base {
            self.event_timestamps
                .iter()
                .map(|instant| {
                    instant
                        .saturating_duration_since(base_instant)
                        .as_nanos()
                        .min(u64::MAX as u128) as u64
                })
                .collect()
        } else {
            Vec::new()
        };

        let mut state = serializer.serialize_struct("TimeWindowPolicy", 4)?;
        state.serialize_field("max_events", &self.max_events)?;
        state.serialize_field("window_duration_nanos", &self.window_duration.as_nanos())?;
        state.serialize_field("timestamps_nanos", &timestamps_nanos)?;
        state.serialize_field("base_timestamp_nanos", &base.map(|_| 0u64))?;
        state.end()
    }
}

#[cfg(feature = "redis-storage")]
impl<'de> Deserialize<'de> for TimeWindowPolicy {
    /// Deserialize TimeWindowPolicy from Redis storage.
    ///
    /// See `Serialize` implementation docs for important notes about timestamp handling.
    /// Timestamps are reconstructed relative to the current time, effectively "resetting"
    /// the time window while preserving relative event spacing.
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::{self, MapAccess, Visitor};

        #[derive(Deserialize)]
        #[serde(field_identifier, rename_all = "snake_case")]
        enum Field {
            MaxEvents,
            WindowDurationNanos,
            TimestampsNanos,
            BaseTimestampNanos,
        }

        struct TimeWindowPolicyVisitor;

        impl<'de> Visitor<'de> for TimeWindowPolicyVisitor {
            type Value = TimeWindowPolicy;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("struct TimeWindowPolicy")
            }

            fn visit_map<V>(self, mut map: V) -> Result<TimeWindowPolicy, V::Error>
            where
                V: MapAccess<'de>,
            {
                let mut max_events = None;
                let mut window_duration_nanos = None;
                let mut timestamps_nanos = None;
                let mut _base_timestamp_nanos = None;

                while let Some(key) = map.next_key()? {
                    match key {
                        Field::MaxEvents => {
                            if max_events.is_some() {
                                return Err(de::Error::duplicate_field("max_events"));
                            }
                            max_events = Some(map.next_value()?);
                        }
                        Field::WindowDurationNanos => {
                            if window_duration_nanos.is_some() {
                                return Err(de::Error::duplicate_field("window_duration_nanos"));
                            }
                            window_duration_nanos = Some(map.next_value()?);
                        }
                        Field::TimestampsNanos => {
                            if timestamps_nanos.is_some() {
                                return Err(de::Error::duplicate_field("timestamps_nanos"));
                            }
                            timestamps_nanos = Some(map.next_value()?);
                        }
                        Field::BaseTimestampNanos => {
                            _base_timestamp_nanos = Some(map.next_value::<Option<u64>>()?);
                        }
                    }
                }

                let max_events =
                    max_events.ok_or_else(|| de::Error::missing_field("max_events"))?;
                let window_duration_nanos: u128 = window_duration_nanos
                    .ok_or_else(|| de::Error::missing_field("window_duration_nanos"))?;
                let timestamps_nanos: Vec<u64> =
                    timestamps_nanos.ok_or_else(|| de::Error::missing_field("timestamps_nanos"))?;

                // Reconstruct Instants relative to current time
                let now = Instant::now();
                let event_timestamps: VecDeque<Instant> = timestamps_nanos
                    .into_iter()
                    .map(|nanos| now.checked_add(Duration::from_nanos(nanos)).unwrap_or(now))
                    .collect();

                Ok(TimeWindowPolicy {
                    max_events,
                    window_duration: Duration::from_nanos(window_duration_nanos as u64),
                    event_timestamps,
                })
            }
        }

        const FIELDS: &[&str] = &[
            "max_events",
            "window_duration_nanos",
            "timestamps_nanos",
            "base_timestamp_nanos",
        ];
        deserializer.deserialize_struct("TimeWindowPolicy", FIELDS, TimeWindowPolicyVisitor)
    }
}

impl TimeWindowPolicy {
    /// Create a new time-window policy.
    ///
    /// # Arguments
    /// * `max_events` - Maximum events allowed in the window (must be > 0)
    /// * `window_duration` - Length of the sliding time window (must be > 0)
    ///
    /// # Errors
    /// Returns `PolicyError::ZeroMaxEvents` if `max_events` is 0.
    /// Returns `PolicyError::ZeroWindowDuration` if `window_duration` is 0.
    pub fn new(max_events: usize, window_duration: Duration) -> Result<Self, PolicyError> {
        if max_events == 0 {
            return Err(PolicyError::ZeroMaxEvents);
        }
        if window_duration.is_zero() {
            return Err(PolicyError::ZeroWindowDuration);
        }
        Ok(Self {
            max_events,
            window_duration,
            event_timestamps: VecDeque::new(),
        })
    }

    /// Remove expired events from the window.
    fn expire_old_events(&mut self, current_time: Instant) {
        while let Some(&oldest) = self.event_timestamps.front() {
            if current_time.saturating_duration_since(oldest) > self.window_duration {
                self.event_timestamps.pop_front();
            } else {
                break;
            }
        }
    }
}

impl RateLimitPolicy for TimeWindowPolicy {
    fn register_event(&mut self, timestamp: Instant) -> PolicyDecision {
        self.expire_old_events(timestamp);

        if self.event_timestamps.len() < self.max_events {
            self.event_timestamps.push_back(timestamp);
            PolicyDecision::Allow
        } else {
            PolicyDecision::Suppress
        }
    }

    fn reset(&mut self) {
        self.event_timestamps.clear();
    }
}

/// Exponential backoff policy.
///
/// Allows events at exponentially increasing intervals: 1st, 2nd, 4th, 8th, 16th, etc.
/// Useful for extremely noisy logs.
///
/// # Example
/// ```
/// use tracing_throttle::{ExponentialBackoffPolicy, RateLimitPolicy};
/// use std::time::Instant;
///
/// let mut policy = ExponentialBackoffPolicy::new();
/// let now = Instant::now();
///
/// assert!(policy.register_event(now).is_allow());  // 1st
/// assert!(policy.register_event(now).is_allow());  // 2nd
/// assert!(policy.register_event(now).is_suppress()); // 3rd - suppressed
/// assert!(policy.register_event(now).is_allow());  // 4th
/// ```
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "redis-storage", derive(Serialize, Deserialize))]
pub struct ExponentialBackoffPolicy {
    event_count: u64,
    next_allowed: u64,
}

impl ExponentialBackoffPolicy {
    /// Create a new exponential backoff policy.
    pub fn new() -> Self {
        Self {
            event_count: 0,
            next_allowed: 1,
        }
    }
}

impl Default for ExponentialBackoffPolicy {
    fn default() -> Self {
        Self::new()
    }
}

impl RateLimitPolicy for ExponentialBackoffPolicy {
    fn register_event(&mut self, _timestamp: Instant) -> PolicyDecision {
        self.event_count += 1;

        if self.event_count == self.next_allowed {
            self.next_allowed = self.next_allowed.saturating_mul(2);
            PolicyDecision::Allow
        } else {
            PolicyDecision::Suppress
        }
    }

    fn reset(&mut self) {
        self.event_count = 0;
        self.next_allowed = 1;
    }
}

/// Token bucket rate limiting policy.
///
/// Implements a token bucket algorithm where:
/// - The bucket holds up to `capacity` tokens
/// - Tokens refill at a constant rate (`refill_rate` tokens per second)
/// - Each event consumes 1 token
/// - Events are suppressed when no tokens are available
///
/// This policy provides:
/// - **Burst tolerance**: Can handle bursts up to `capacity` events
/// - **Natural recovery**: Tokens automatically refill over time
/// - **Smooth rate limiting**: Sustained load is limited to `refill_rate`
/// - **Forgiveness**: After quiet periods, full capacity is restored
///
/// # Example
/// ```
/// use tracing_throttle::{TokenBucketPolicy, RateLimitPolicy};
/// use std::time::{Duration, Instant};
///
/// // Bucket with capacity 100, refills at 10 tokens/sec
/// let mut policy = TokenBucketPolicy::new(100.0, 10.0).unwrap();
/// let start = Instant::now();
///
/// // Can burst up to 100 events immediately
/// for _ in 0..100 {
///     assert!(policy.register_event(start).is_allow());
/// }
///
/// // 101st event is suppressed (no tokens left)
/// assert!(policy.register_event(start).is_suppress());
///
/// // After 1 second, 10 more tokens available
/// let later = start + Duration::from_secs(1);
/// for _ in 0..10 {
///     assert!(policy.register_event(later).is_allow());
/// }
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct TokenBucketPolicy {
    /// Maximum number of tokens the bucket can hold
    capacity: f64,
    /// Rate at which tokens are added (tokens per second)
    refill_rate: f64,
    /// Current number of tokens in the bucket
    tokens: f64,
    /// Last time the bucket was refilled
    last_refill: Option<Instant>,
}

#[cfg(feature = "redis-storage")]
impl Serialize for TokenBucketPolicy {
    /// Serialize TokenBucketPolicy for Redis storage.
    ///
    /// # Important: last_refill is NOT serialized
    ///
    /// The `last_refill` field (`Option<Instant>`) is intentionally not serialized because:
    ///
    /// 1. Instant cannot be serialized (system-dependent, no epoch)
    /// 2. After deserialization, the first event will trigger a refill calculation
    /// 3. The token count is preserved, so the bucket state is mostly maintained
    ///
    /// # Implications
    ///
    /// When a TokenBucketPolicy is loaded from Redis:
    /// - Current token count is restored accurately
    /// - `last_refill` is set to `None`
    /// - On first event after reload, tokens will refill based on time since "now"
    /// - This may allow a small burst beyond the intended rate immediately after reload
    ///
    /// **Trade-off**: This is acceptable because the impact is bounded by the bucket
    /// capacity and only affects the first event after reload.
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;

        let mut state = serializer.serialize_struct("TokenBucketPolicy", 4)?;
        state.serialize_field("capacity", &self.capacity)?;
        state.serialize_field("refill_rate", &self.refill_rate)?;
        state.serialize_field("tokens", &self.tokens)?;
        // We intentionally don't serialize last_refill - it will be set on first use after deserialization
        // This is acceptable because the policy will refill based on the new timestamp
        state.serialize_field("has_last_refill", &self.last_refill.is_some())?;
        state.end()
    }
}

#[cfg(feature = "redis-storage")]
impl<'de> Deserialize<'de> for TokenBucketPolicy {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::{self, MapAccess, Visitor};

        #[derive(Deserialize)]
        #[serde(field_identifier, rename_all = "snake_case")]
        enum Field {
            Capacity,
            RefillRate,
            Tokens,
            HasLastRefill,
        }

        struct TokenBucketPolicyVisitor;

        impl<'de> Visitor<'de> for TokenBucketPolicyVisitor {
            type Value = TokenBucketPolicy;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("struct TokenBucketPolicy")
            }

            fn visit_map<V>(self, mut map: V) -> Result<TokenBucketPolicy, V::Error>
            where
                V: MapAccess<'de>,
            {
                let mut capacity = None;
                let mut refill_rate = None;
                let mut tokens = None;
                let mut has_last_refill = None;

                while let Some(key) = map.next_key()? {
                    match key {
                        Field::Capacity => {
                            if capacity.is_some() {
                                return Err(de::Error::duplicate_field("capacity"));
                            }
                            capacity = Some(map.next_value()?);
                        }
                        Field::RefillRate => {
                            if refill_rate.is_some() {
                                return Err(de::Error::duplicate_field("refill_rate"));
                            }
                            refill_rate = Some(map.next_value()?);
                        }
                        Field::Tokens => {
                            if tokens.is_some() {
                                return Err(de::Error::duplicate_field("tokens"));
                            }
                            tokens = Some(map.next_value()?);
                        }
                        Field::HasLastRefill => {
                            has_last_refill = Some(map.next_value()?);
                        }
                    }
                }

                let capacity = capacity.ok_or_else(|| de::Error::missing_field("capacity"))?;
                let refill_rate =
                    refill_rate.ok_or_else(|| de::Error::missing_field("refill_rate"))?;
                let tokens = tokens.ok_or_else(|| de::Error::missing_field("tokens"))?;
                let _has_last_refill = has_last_refill.unwrap_or(false);

                // Set last_refill to None - it will be set on first use after deserialization
                // This is a safe approach: the bucket will refill based on the next timestamp
                Ok(TokenBucketPolicy {
                    capacity,
                    refill_rate,
                    tokens,
                    last_refill: None,
                })
            }
        }

        const FIELDS: &[&str] = &["capacity", "refill_rate", "tokens", "has_last_refill"];
        deserializer.deserialize_struct("TokenBucketPolicy", FIELDS, TokenBucketPolicyVisitor)
    }
}

impl TokenBucketPolicy {
    /// Create a new token bucket policy.
    ///
    /// # Arguments
    /// * `capacity` - Maximum tokens in the bucket (burst size, must be > 0)
    /// * `refill_rate` - Tokens added per second (sustained rate, must be > 0)
    ///
    /// # Errors
    /// Returns `PolicyError::ZeroCapacity` if `capacity` is 0 or negative.
    /// Returns `PolicyError::ZeroRefillRate` if `refill_rate` is 0 or negative.
    ///
    /// # Example
    /// ```
    /// use tracing_throttle::TokenBucketPolicy;
    ///
    /// // 100 token burst, refills at 10/sec
    /// let policy = TokenBucketPolicy::new(100.0, 10.0).unwrap();
    /// ```
    pub fn new(capacity: f64, refill_rate: f64) -> Result<Self, PolicyError> {
        if capacity <= 0.0 {
            return Err(PolicyError::ZeroCapacity);
        }
        if refill_rate <= 0.0 {
            return Err(PolicyError::ZeroRefillRate);
        }

        Ok(Self {
            capacity,
            refill_rate,
            tokens: capacity,
            last_refill: None,
        })
    }

    /// Refill tokens based on elapsed time since last refill.
    ///
    /// Handles clock adjustments gracefully - if time goes backwards,
    /// we simply reset the refill timestamp without adding tokens.
    fn refill(&mut self, now: Instant) {
        if let Some(last) = self.last_refill {
            // Handle time going backwards (NTP adjustments, VM migrations, etc.)
            if now < last {
                self.last_refill = Some(now);
                return;
            }

            let elapsed = now.duration_since(last).as_secs_f64();
            let new_tokens = elapsed * self.refill_rate;
            self.tokens = (self.tokens + new_tokens).min(self.capacity);
        }
        self.last_refill = Some(now);
    }
}

impl RateLimitPolicy for TokenBucketPolicy {
    fn register_event(&mut self, timestamp: Instant) -> PolicyDecision {
        // Refill tokens based on time elapsed
        self.refill(timestamp);

        // Check if we have a token available
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            PolicyDecision::Allow
        } else {
            PolicyDecision::Suppress
        }
    }

    fn reset(&mut self) {
        self.tokens = self.capacity;
        self.last_refill = None;
    }
}

/// Convenience enum for common policy types.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "redis-storage", derive(Serialize, Deserialize))]
pub enum Policy {
    /// Count-based policy
    CountBased(CountBasedPolicy),
    /// Time-window policy
    TimeWindow(TimeWindowPolicy),
    /// Exponential backoff policy
    ExponentialBackoff(ExponentialBackoffPolicy),
    /// Token bucket policy
    TokenBucket(TokenBucketPolicy),
}

impl Policy {
    /// Create a count-based policy.
    ///
    /// # Errors
    /// Returns `PolicyError::ZeroMaxCount` if `max_count` is 0.
    pub fn count_based(max_count: usize) -> Result<Self, PolicyError> {
        Ok(Policy::CountBased(CountBasedPolicy::new(max_count)?))
    }

    /// Create a time-window policy.
    ///
    /// # Errors
    /// Returns `PolicyError::ZeroMaxEvents` if `max_events` is 0.
    /// Returns `PolicyError::ZeroWindowDuration` if `window` is 0.
    pub fn time_window(max_events: usize, window: Duration) -> Result<Self, PolicyError> {
        Ok(Policy::TimeWindow(TimeWindowPolicy::new(
            max_events, window,
        )?))
    }

    /// Create an exponential backoff policy.
    ///
    /// This policy has no configurable parameters and cannot fail.
    pub fn exponential_backoff() -> Self {
        Policy::ExponentialBackoff(ExponentialBackoffPolicy::new())
    }

    /// Create a token bucket policy.
    ///
    /// # Arguments
    /// * `capacity` - Maximum tokens (burst size, must be > 0)
    /// * `refill_rate` - Tokens per second (sustained rate, must be > 0)
    ///
    /// # Errors
    /// Returns `PolicyError::ZeroCapacity` if `capacity` is 0 or negative.
    /// Returns `PolicyError::ZeroRefillRate` if `refill_rate` is 0 or negative.
    ///
    /// # Example
    /// ```
    /// use tracing_throttle::Policy;
    ///
    /// // Allow bursts of 100, refill at 10/sec
    /// let policy = Policy::token_bucket(100.0, 10.0).unwrap();
    /// ```
    pub fn token_bucket(capacity: f64, refill_rate: f64) -> Result<Self, PolicyError> {
        Ok(Policy::TokenBucket(TokenBucketPolicy::new(
            capacity,
            refill_rate,
        )?))
    }
}

impl RateLimitPolicy for Policy {
    fn register_event(&mut self, timestamp: Instant) -> PolicyDecision {
        match self {
            Policy::CountBased(p) => p.register_event(timestamp),
            Policy::TimeWindow(p) => p.register_event(timestamp),
            Policy::ExponentialBackoff(p) => p.register_event(timestamp),
            Policy::TokenBucket(p) => p.register_event(timestamp),
        }
    }

    fn reset(&mut self) {
        match self {
            Policy::CountBased(p) => p.reset(),
            Policy::TimeWindow(p) => p.reset(),
            Policy::ExponentialBackoff(p) => p.reset(),
            Policy::TokenBucket(p) => p.reset(),
        }
    }
}

impl PolicyDecision {
    /// Check if this decision is Allow.
    pub fn is_allow(&self) -> bool {
        matches!(self, PolicyDecision::Allow)
    }

    /// Check if this decision is Suppress.
    pub fn is_suppress(&self) -> bool {
        matches!(self, PolicyDecision::Suppress)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_count_based_policy() {
        let mut policy = CountBasedPolicy::new(3).unwrap();
        let now = Instant::now();

        assert_eq!(policy.register_event(now), PolicyDecision::Allow);
        assert_eq!(policy.register_event(now), PolicyDecision::Allow);
        assert_eq!(policy.register_event(now), PolicyDecision::Allow);
        assert_eq!(policy.register_event(now), PolicyDecision::Suppress);
        assert_eq!(policy.register_event(now), PolicyDecision::Suppress);

        policy.reset();
        assert_eq!(policy.register_event(now), PolicyDecision::Allow);
    }

    #[test]
    fn test_time_window_policy() {
        let mut policy = TimeWindowPolicy::new(2, Duration::from_secs(1)).unwrap();
        let now = Instant::now();

        assert_eq!(policy.register_event(now), PolicyDecision::Allow);
        assert_eq!(policy.register_event(now), PolicyDecision::Allow);
        assert_eq!(policy.register_event(now), PolicyDecision::Suppress);

        // After window expires, should allow again
        let later = now + Duration::from_secs(2);
        assert_eq!(policy.register_event(later), PolicyDecision::Allow);
    }

    #[test]
    fn test_exponential_backoff_policy() {
        let mut policy = ExponentialBackoffPolicy::new();
        let now = Instant::now();

        // 1st allowed
        assert_eq!(policy.register_event(now), PolicyDecision::Allow);
        // 2nd allowed
        assert_eq!(policy.register_event(now), PolicyDecision::Allow);
        // 3rd suppressed
        assert_eq!(policy.register_event(now), PolicyDecision::Suppress);
        // 4th allowed
        assert_eq!(policy.register_event(now), PolicyDecision::Allow);
        // 5th, 6th, 7th suppressed
        assert_eq!(policy.register_event(now), PolicyDecision::Suppress);
        assert_eq!(policy.register_event(now), PolicyDecision::Suppress);
        assert_eq!(policy.register_event(now), PolicyDecision::Suppress);
        // 8th allowed
        assert_eq!(policy.register_event(now), PolicyDecision::Allow);
    }

    #[test]
    fn test_policy_enum() {
        let mut policy = Policy::count_based(2).unwrap();
        let now = Instant::now();

        assert!(policy.register_event(now).is_allow());
        assert!(policy.register_event(now).is_allow());
        assert!(policy.register_event(now).is_suppress());
    }

    // Edge case tests
    #[test]
    fn test_count_based_policy_zero_limit() {
        // Zero limit should be rejected
        let result = CountBasedPolicy::new(0);
        assert_eq!(result, Err(PolicyError::ZeroMaxCount));
    }

    #[test]
    fn test_count_based_policy_one_limit() {
        let mut policy = CountBasedPolicy::new(1).unwrap();
        let now = Instant::now();

        // Only first event allowed
        assert_eq!(policy.register_event(now), PolicyDecision::Allow);
        assert_eq!(policy.register_event(now), PolicyDecision::Suppress);
        assert_eq!(policy.register_event(now), PolicyDecision::Suppress);
    }

    #[test]
    fn test_count_based_policy_reset() {
        let mut policy = CountBasedPolicy::new(2).unwrap();
        let now = Instant::now();

        // Use up the limit
        assert_eq!(policy.register_event(now), PolicyDecision::Allow);
        assert_eq!(policy.register_event(now), PolicyDecision::Allow);
        assert_eq!(policy.register_event(now), PolicyDecision::Suppress);

        // Reset should restore the limit
        policy.reset();
        assert_eq!(policy.register_event(now), PolicyDecision::Allow);
        assert_eq!(policy.register_event(now), PolicyDecision::Allow);
        assert_eq!(policy.register_event(now), PolicyDecision::Suppress);
    }

    #[test]
    fn test_time_window_policy_zero_duration() {
        // Zero duration should be rejected
        let result = TimeWindowPolicy::new(2, Duration::from_secs(0));
        assert_eq!(result, Err(PolicyError::ZeroWindowDuration));
    }

    #[test]
    fn test_time_window_policy_rapid_events() {
        let mut policy = TimeWindowPolicy::new(3, Duration::from_millis(100)).unwrap();
        let now = Instant::now();

        // Rapid fire events
        for i in 0..10 {
            let decision = policy.register_event(now);
            if i < 3 {
                assert_eq!(
                    decision,
                    PolicyDecision::Allow,
                    "Event {} should be allowed",
                    i
                );
            } else {
                assert_eq!(
                    decision,
                    PolicyDecision::Suppress,
                    "Event {} should be suppressed",
                    i
                );
            }
        }
    }

    #[test]
    fn test_time_window_policy_reset() {
        let mut policy = TimeWindowPolicy::new(2, Duration::from_secs(60)).unwrap();
        let now = Instant::now();

        // Use up limit
        assert_eq!(policy.register_event(now), PolicyDecision::Allow);
        assert_eq!(policy.register_event(now), PolicyDecision::Allow);
        assert_eq!(policy.register_event(now), PolicyDecision::Suppress);

        // Reset should clear the window
        policy.reset();
        assert_eq!(policy.register_event(now), PolicyDecision::Allow);
    }

    #[test]
    fn test_exponential_backoff_large_count() {
        let mut policy = ExponentialBackoffPolicy::new();
        let now = Instant::now();

        let expected_allowed = [0, 1, 3, 7, 15, 31, 63]; // 0-indexed: 1st, 2nd, 4th, 8th, 16th, 32nd, 64th

        for i in 0..100 {
            let decision = policy.register_event(now);
            if expected_allowed.contains(&i) {
                assert_eq!(
                    decision,
                    PolicyDecision::Allow,
                    "Event {} should be allowed",
                    i + 1
                );
            } else {
                assert_eq!(
                    decision,
                    PolicyDecision::Suppress,
                    "Event {} should be suppressed",
                    i + 1
                );
            }
        }
    }

    #[test]
    fn test_exponential_backoff_reset() {
        let mut policy = ExponentialBackoffPolicy::new();
        let now = Instant::now();

        // Progress through first few events
        assert_eq!(policy.register_event(now), PolicyDecision::Allow); // 1st
        assert_eq!(policy.register_event(now), PolicyDecision::Allow); // 2nd
        assert_eq!(policy.register_event(now), PolicyDecision::Suppress); // 3rd

        // Reset should start over
        policy.reset();
        assert_eq!(policy.register_event(now), PolicyDecision::Allow); // 1st again
    }

    // Token Bucket Policy Tests
    #[test]
    fn test_token_bucket_basic_consumption() {
        let mut policy = TokenBucketPolicy::new(3.0, 1.0).unwrap();
        let now = Instant::now();

        // Should allow up to capacity
        assert_eq!(policy.register_event(now), PolicyDecision::Allow);
        assert_eq!(policy.register_event(now), PolicyDecision::Allow);
        assert_eq!(policy.register_event(now), PolicyDecision::Allow);
        // Bucket empty, should suppress
        assert_eq!(policy.register_event(now), PolicyDecision::Suppress);
        assert_eq!(policy.register_event(now), PolicyDecision::Suppress);
    }

    #[test]
    fn test_token_bucket_refill_over_time() {
        let mut policy = TokenBucketPolicy::new(10.0, 10.0).unwrap(); // 10 tokens/sec
        let now = Instant::now();

        // Use all tokens
        for _ in 0..10 {
            assert_eq!(policy.register_event(now), PolicyDecision::Allow);
        }
        assert_eq!(policy.register_event(now), PolicyDecision::Suppress);

        // Wait 0.5 seconds - should get 5 tokens back
        let later = now + Duration::from_millis(500);
        for i in 0..5 {
            assert_eq!(
                policy.register_event(later),
                PolicyDecision::Allow,
                "Event {} should be allowed after refill",
                i
            );
        }
        assert_eq!(policy.register_event(later), PolicyDecision::Suppress);
    }

    #[test]
    fn test_token_bucket_burst_tolerance() {
        let mut policy = TokenBucketPolicy::new(100.0, 1.0).unwrap();
        let now = Instant::now();

        // Can burst up to full capacity immediately
        for i in 0..100 {
            assert_eq!(
                policy.register_event(now),
                PolicyDecision::Allow,
                "Event {} in burst should be allowed",
                i
            );
        }
        // Then rate limited
        assert_eq!(policy.register_event(now), PolicyDecision::Suppress);
    }

    #[test]
    fn test_token_bucket_sustained_rate() {
        let mut policy = TokenBucketPolicy::new(10.0, 10.0).unwrap(); // 10/sec sustained, 10 capacity
        let now = Instant::now();

        // Use all tokens
        for _ in 0..10 {
            assert_eq!(policy.register_event(now), PolicyDecision::Allow);
        }
        assert_eq!(policy.register_event(now), PolicyDecision::Suppress);

        // Wait 1 second - should get 10 tokens back (capped at capacity)
        let later = now + Duration::from_secs(1);
        for i in 0..10 {
            assert_eq!(
                policy.register_event(later),
                PolicyDecision::Allow,
                "Event {} after 1s should be allowed",
                i
            );
        }
        assert_eq!(policy.register_event(later), PolicyDecision::Suppress);

        // Wait 0.5 seconds - should get 5 tokens
        let even_later = later + Duration::from_millis(500);
        for i in 0..5 {
            assert_eq!(
                policy.register_event(even_later),
                PolicyDecision::Allow,
                "Event {} after 0.5s should be allowed",
                i
            );
        }
        assert_eq!(policy.register_event(even_later), PolicyDecision::Suppress);
    }

    #[test]
    fn test_token_bucket_recovery_after_quiet() {
        let mut policy = TokenBucketPolicy::new(5.0, 2.0).unwrap();
        let now = Instant::now();

        // Use all tokens
        for _ in 0..5 {
            policy.register_event(now);
        }
        assert_eq!(policy.register_event(now), PolicyDecision::Suppress);

        // Wait long enough to fully recover
        let much_later = now + Duration::from_secs(10);
        // Should be back to full capacity (5 tokens)
        for i in 0..5 {
            assert_eq!(
                policy.register_event(much_later),
                PolicyDecision::Allow,
                "Event {} after recovery should be allowed",
                i
            );
        }
        assert_eq!(policy.register_event(much_later), PolicyDecision::Suppress);
    }

    #[test]
    fn test_token_bucket_fractional_refill() {
        let mut policy = TokenBucketPolicy::new(10.0, 0.5).unwrap(); // 0.5 tokens/sec
        let now = Instant::now();

        // Use all tokens
        for _ in 0..10 {
            policy.register_event(now);
        }
        assert_eq!(policy.register_event(now), PolicyDecision::Suppress);

        // Wait 3 seconds - should get 1.5 tokens (only 1 usable)
        let later = now + Duration::from_secs(3);
        assert_eq!(policy.register_event(later), PolicyDecision::Allow);
        assert_eq!(policy.register_event(later), PolicyDecision::Suppress); // 0.5 tokens left, not enough

        // Wait 1 more second - should have 1.5 tokens now
        let even_later = later + Duration::from_secs(1);
        assert_eq!(policy.register_event(even_later), PolicyDecision::Allow);
        assert_eq!(policy.register_event(even_later), PolicyDecision::Suppress);
    }

    #[test]
    fn test_token_bucket_reset() {
        let mut policy = TokenBucketPolicy::new(5.0, 1.0).unwrap();
        let now = Instant::now();

        // Use all tokens
        for _ in 0..5 {
            policy.register_event(now);
        }
        assert_eq!(policy.register_event(now), PolicyDecision::Suppress);

        // Reset should restore full capacity
        policy.reset();
        for i in 0..5 {
            assert_eq!(
                policy.register_event(now),
                PolicyDecision::Allow,
                "Event {} after reset should be allowed",
                i
            );
        }
        assert_eq!(policy.register_event(now), PolicyDecision::Suppress);
    }

    #[test]
    fn test_token_bucket_capacity_cap() {
        let mut policy = TokenBucketPolicy::new(5.0, 10.0).unwrap();
        let now = Instant::now();

        // Use some tokens
        for _ in 0..3 {
            policy.register_event(now);
        }

        // Wait long time - tokens should cap at capacity (5), not grow unbounded
        let much_later = now + Duration::from_secs(100);
        for i in 0..5 {
            assert_eq!(
                policy.register_event(much_later),
                PolicyDecision::Allow,
                "Event {} should be allowed (capped at capacity)",
                i
            );
        }
        assert_eq!(policy.register_event(much_later), PolicyDecision::Suppress);
    }

    #[test]
    fn test_token_bucket_zero_capacity() {
        let result = TokenBucketPolicy::new(0.0, 1.0);
        assert_eq!(result, Err(PolicyError::ZeroCapacity));
    }

    #[test]
    fn test_token_bucket_negative_capacity() {
        let result = TokenBucketPolicy::new(-5.0, 1.0);
        assert_eq!(result, Err(PolicyError::ZeroCapacity));
    }

    #[test]
    fn test_token_bucket_zero_refill_rate() {
        let result = TokenBucketPolicy::new(10.0, 0.0);
        assert_eq!(result, Err(PolicyError::ZeroRefillRate));
    }

    #[test]
    fn test_token_bucket_negative_refill_rate() {
        let result = TokenBucketPolicy::new(10.0, -2.0);
        assert_eq!(result, Err(PolicyError::ZeroRefillRate));
    }

    #[test]
    fn test_token_bucket_policy_enum() {
        let mut policy = Policy::token_bucket(5.0, 2.0).unwrap();
        let now = Instant::now();

        // Test via Policy enum
        for i in 0..5 {
            assert!(
                policy.register_event(now).is_allow(),
                "Event {} should be allowed",
                i
            );
        }
        assert!(policy.register_event(now).is_suppress());

        // Test reset via enum
        policy.reset();
        assert!(policy.register_event(now).is_allow());
    }

    #[test]
    fn test_token_bucket_incremental_refill() {
        let mut policy = TokenBucketPolicy::new(1.0, 10.0).unwrap(); // 10 tokens/sec, 1 max
        let now = Instant::now();

        // Use initial token
        assert_eq!(policy.register_event(now), PolicyDecision::Allow);
        assert_eq!(policy.register_event(now), PolicyDecision::Suppress);

        // Incremental refills - 100ms = 1 token
        let t1 = now + Duration::from_millis(100);
        assert_eq!(policy.register_event(t1), PolicyDecision::Allow);
        assert_eq!(policy.register_event(t1), PolicyDecision::Suppress);

        let t2 = t1 + Duration::from_millis(100);
        assert_eq!(policy.register_event(t2), PolicyDecision::Allow);
        assert_eq!(policy.register_event(t2), PolicyDecision::Suppress);
    }

    #[test]
    fn test_token_bucket_same_timestamp_multiple_events() {
        // Regression test: multiple events at the same timestamp should not refill
        let mut policy = TokenBucketPolicy::new(5.0, 2.0).unwrap();
        let start = Instant::now();

        // First burst at t=0: use all 5 tokens
        for i in 0..5 {
            assert_eq!(
                policy.register_event(start),
                PolicyDecision::Allow,
                "Event {} should be allowed",
                i
            );
        }

        // Events 6,7,8 at t=0 should be suppressed (no tokens left)
        for i in 5..8 {
            assert_eq!(
                policy.register_event(start),
                PolicyDecision::Suppress,
                "Event {} should be suppressed (no tokens)",
                i
            );
        }

        // After 1 second, should have refilled 2 tokens
        let t1 = start + Duration::from_secs(1);

        // Events at t=1s: should allow exactly 2
        assert_eq!(
            policy.register_event(t1),
            PolicyDecision::Allow,
            "First event after 1s should be allowed"
        );
        assert_eq!(
            policy.register_event(t1),
            PolicyDecision::Allow,
            "Second event after 1s should be allowed"
        );

        // Third event at t=1s should be suppressed (only refilled 2 tokens)
        assert_eq!(
            policy.register_event(t1),
            PolicyDecision::Suppress,
            "Third event after 1s should be suppressed (only 2 tokens refilled)"
        );

        // Fourth and fifth should also be suppressed
        assert_eq!(policy.register_event(t1), PolicyDecision::Suppress);
        assert_eq!(policy.register_event(t1), PolicyDecision::Suppress);
    }

    #[test]
    fn test_token_bucket_time_goes_backwards() {
        let mut policy = TokenBucketPolicy::new(10.0, 5.0).unwrap();
        let now = Instant::now();

        // Use 5 tokens
        for _ in 0..5 {
            assert_eq!(policy.register_event(now), PolicyDecision::Allow);
        }
        // 5 tokens remaining

        // Time goes forward - should refill 5 tokens (now have 10)
        let future = now + Duration::from_secs(1);
        for _ in 0..10 {
            assert_eq!(policy.register_event(future), PolicyDecision::Allow);
        }
        // 0 tokens remaining after using all 10

        // Time goes backwards (NTP correction, VM migration, etc.)
        // Should NOT panic and should NOT add/remove tokens
        let past = now + Duration::from_millis(500);
        assert!(past < future, "Test setup: past must be before future");

        // Should still have 0 tokens (time went backwards, no refill)
        assert_eq!(
            policy.register_event(past),
            PolicyDecision::Suppress,
            "Should suppress when no tokens available after time went backwards"
        );

        // Time moves forward again normally (1 second after 'past')
        let future2 = past + Duration::from_secs(1);
        // Should refill 5 tokens based on elapsed time from 'past'
        for i in 0..5 {
            assert_eq!(
                policy.register_event(future2),
                PolicyDecision::Allow,
                "Token {} should be available after normal time progression",
                i
            );
        }

        // 6th should be suppressed
        assert_eq!(policy.register_event(future2), PolicyDecision::Suppress);
    }

    #[test]
    fn test_time_window_with_many_events() {
        // Fill time window with maximum events
        let mut policy = TimeWindowPolicy::new(100, Duration::from_secs(60)).unwrap();
        let now = Instant::now();

        // Add 100 events
        for i in 0..100 {
            let timestamp = now + Duration::from_millis(i * 10);
            policy.register_event(timestamp);
        }

        // Verify window is full
        assert_eq!(
            policy.register_event(now + Duration::from_millis(1000)),
            PolicyDecision::Suppress
        );

        // After window expires, should allow again
        let later = now + Duration::from_secs(70);
        assert_eq!(policy.register_event(later), PolicyDecision::Allow);
    }
}
