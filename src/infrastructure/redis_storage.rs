//! Redis-backed storage implementation.
//!
//! Provides a distributed storage backend using Redis, allowing rate limiting
//! state to be shared across multiple application instances.
//!
//! ## Architecture
//!
//! The Redis storage uses a simple key-value model:
//! - Keys: Signature hash (u64) as string with configurable prefix
//! - Values: Serialized EventState (bincode format)
//! - TTL: Automatic expiration for inactive signatures
//!
//! ## Features
//!
//! - Automatic expiration (TTL) for unused signatures
//! - Connection pooling via `redis::aio::ConnectionManager`
//! - Async-only interface (requires `tokio` runtime)
//! - Fail-safe operation: continues working even if Redis is unavailable
//! - Warning logs for Redis errors (doesn't crash on failures)
//!
//! ## Important Limitations
//!
//! ### 1. Metrics Limitations
//!
//! - `len()` always returns 0 (signature count unavailable)
//! - `is_empty()` always returns false
//! - These would require expensive SCAN operations
//! - Use Redis monitoring tools for accurate metrics instead
//!
//! ### 2. Policy Serialization Trade-offs
//!
//! - **TimeWindowPolicy**: Timestamps reset to current time on reload
//! - **TokenBucketPolicy**: `last_refill` not persisted, may allow small burst after reload
//! - See policy documentation for detailed implications
//!
//! ### 3. Error Handling
//!
//! - Redis failures are logged as warnings
//! - Operations continue with local state if Redis unavailable
//! - This ensures observability isn't lost during Redis outages
//!
//! ## Example
//!
//! ```rust,ignore
//! use tracing_throttle::{RedisStorage, RedisStorageConfig, TracingRateLimitLayer, Policy, SystemClock};
//! use std::sync::Arc;
//! use std::time::Duration;
//!
//! #[tokio::main]
//! async fn main() {
//!     let config = RedisStorageConfig {
//!         key_prefix: "tracing_throttle:".to_string(),
//!         ttl: Duration::from_secs(3600),
//!     };
//!
//!     let storage = RedisStorage::connect_with_config("redis://127.0.0.1/", config)
//!         .await
//!         .expect("Failed to connect to Redis");
//!
//!     let policy = Policy::token_bucket(100.0, 10.0).unwrap();
//!     let clock = Arc::new(SystemClock::new());
//!     let layer = TracingRateLimitLayer::with_storage(storage, policy, clock);
//! }
//! ```

use crate::application::ports::Storage;
use crate::application::registry::EventState;
use crate::domain::signature::EventSignature;
use redis::aio::ConnectionManager;
use redis::{AsyncCommands, Client, RedisError};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// Serializable version of EventState for Redis storage.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SerializableEventState {
    policy: crate::domain::policy::Policy,
    suppressed_count: usize,
    first_suppressed_secs: u64,
    first_suppressed_nanos: u32,
    last_suppressed_secs: u64,
    last_suppressed_nanos: u32,
}

impl SerializableEventState {
    /// Convert from runtime EventState to serializable form.
    fn from_event_state(state: &EventState, base_instant: Instant) -> Self {
        let counter = state.counter.snapshot();

        // Convert Instant to Duration since base_instant for serialization
        let first_duration = counter.first_suppressed.duration_since(base_instant);
        let last_duration = counter.last_suppressed.duration_since(base_instant);

        Self {
            policy: state.policy.clone(),
            suppressed_count: counter.suppressed_count,
            first_suppressed_secs: first_duration.as_secs(),
            first_suppressed_nanos: first_duration.subsec_nanos(),
            last_suppressed_secs: last_duration.as_secs(),
            last_suppressed_nanos: last_duration.subsec_nanos(),
        }
    }

    /// Convert to runtime EventState.
    fn to_event_state(&self, base_instant: Instant) -> EventState {
        let first_suppressed =
            base_instant + Duration::new(self.first_suppressed_secs, self.first_suppressed_nanos);
        let last_suppressed =
            base_instant + Duration::new(self.last_suppressed_secs, self.last_suppressed_nanos);

        EventState::from_snapshot(
            self.policy.clone(),
            self.suppressed_count,
            first_suppressed,
            last_suppressed,
        )
    }
}

/// Configuration for Redis storage.
#[derive(Debug, Clone)]
pub struct RedisStorageConfig {
    /// TTL for inactive signatures (default: 1 hour)
    pub ttl: Duration,
    /// Key prefix for Redis keys (default: "tracing-throttle:")
    pub key_prefix: String,
}

impl Default for RedisStorageConfig {
    fn default() -> Self {
        Self {
            ttl: Duration::from_secs(3600),
            key_prefix: "tracing-throttle:".to_string(),
        }
    }
}

/// Redis-backed storage for distributed rate limiting.
///
/// This storage implementation allows multiple application instances
/// to share rate limiting state via Redis.
pub struct RedisStorage {
    connection: Arc<RwLock<ConnectionManager>>,
    config: RedisStorageConfig,
    /// Base instant for timestamp serialization
    base_instant: Instant,
}

impl fmt::Debug for RedisStorage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RedisStorage")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl RedisStorage {
    /// Connect to Redis with default configuration.
    ///
    /// # Arguments
    /// * `url` - Redis connection URL (e.g., "redis://127.0.0.1/")
    ///
    /// # Errors
    /// Returns error if connection fails.
    pub async fn connect(url: &str) -> Result<Self, RedisError> {
        Self::connect_with_config(url, RedisStorageConfig::default()).await
    }

    /// Connect to Redis with custom configuration.
    ///
    /// # Arguments
    /// * `url` - Redis connection URL
    /// * `config` - Storage configuration
    ///
    /// # Errors
    /// Returns error if connection fails.
    pub async fn connect_with_config(
        url: &str,
        config: RedisStorageConfig,
    ) -> Result<Self, RedisError> {
        let client = Client::open(url)?;
        let connection = ConnectionManager::new(client).await?;

        Ok(Self {
            connection: Arc::new(RwLock::new(connection)),
            config,
            base_instant: Instant::now(),
        })
    }

    /// Get the Redis key for a signature.
    fn key(&self, signature: &EventSignature) -> String {
        format!("{}{}", self.config.key_prefix, signature)
    }

    /// Get a value from Redis, deserializing it.
    async fn get(&self, signature: &EventSignature) -> Result<Option<EventState>, RedisError> {
        let key = self.key(signature);
        let mut conn = self.connection.write().await;

        let bytes: Option<Vec<u8>> = conn.get(&key).await?;

        if let Some(bytes) = bytes {
            if let Ok(serializable) = bincode::deserialize::<SerializableEventState>(&bytes) {
                Ok(Some(serializable.to_event_state(self.base_instant)))
            } else {
                // Corrupted data, delete it
                let _: () = conn.del(&key).await?;
                Ok(None)
            }
        } else {
            Ok(None)
        }
    }

    /// Set a value in Redis, serializing it with TTL.
    async fn set(&self, signature: &EventSignature, state: &EventState) -> Result<(), RedisError> {
        let key = self.key(signature);
        let serializable = SerializableEventState::from_event_state(state, self.base_instant);

        if let Ok(bytes) = bincode::serialize(&serializable) {
            let mut conn = self.connection.write().await;
            let ttl_secs = self.config.ttl.as_secs();

            conn.set_ex::<_, _, ()>(&key, bytes, ttl_secs).await?;
        }

        Ok(())
    }
}

impl Clone for RedisStorage {
    fn clone(&self) -> Self {
        Self {
            connection: Arc::clone(&self.connection),
            config: self.config.clone(),
            base_instant: self.base_instant,
        }
    }
}

impl Storage<EventSignature, EventState> for RedisStorage {
    /// Access or create an entry in Redis storage.
    ///
    /// This implementation bridges the sync `Storage` trait with async Redis operations.
    /// It handles both async contexts (using `block_in_place`) and sync contexts
    /// (creating a temporary runtime).
    ///
    /// # Error Handling
    ///
    /// - Redis GET failures are treated as cache misses (factory is called)
    /// - Redis SET failures are logged as warnings but don't fail the operation
    /// - This ensures the rate limiter continues operating even if Redis is unavailable
    fn with_entry_mut<F, R>(
        &self,
        key: EventSignature,
        factory: impl FnOnce() -> EventState,
        accessor: F,
    ) -> R
    where
        F: FnOnce(&mut EventState) -> R,
    {
        // Redis operations must be async, but Storage trait is sync
        // Try to get current runtime handle, or create a new runtime if needed
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            // We're in an async context - spawn a blocking task
            tokio::task::block_in_place(|| {
                handle.block_on(async {
                    let mut state = match self.get(&key).await {
                        Ok(Some(state)) => state,
                        Ok(None) | Err(_) => factory(),
                    };
                    let result = accessor(&mut state);

                    // Attempt to persist updated state
                    if let Err(e) = self.set(&key, &state).await {
                        tracing::warn!(
                            error = %e,
                            signature = %key.as_hash(),
                            "Failed to persist event state to Redis"
                        );
                    }

                    result
                })
            })
        } else {
            // Not in async context - create runtime
            let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
            rt.block_on(async {
                let mut state = match self.get(&key).await {
                    Ok(Some(state)) => state,
                    Ok(None) | Err(_) => factory(),
                };
                let result = accessor(&mut state);

                // Attempt to persist updated state
                if let Err(e) = self.set(&key, &state).await {
                    tracing::warn!(
                        error = %e,
                        signature = %key.as_hash(),
                        "Failed to persist event state to Redis"
                    );
                }

                result
            })
        }
    }

    /// Returns the number of tracked signatures.
    ///
    /// # Limitation
    ///
    /// **This always returns 0 for RedisStorage.**
    ///
    /// Getting an accurate count would require a full SCAN of all keys matching our
    /// prefix, which would be expensive for large keyspaces. Since this metric is
    /// primarily informational (used for monitoring signature count), we return 0
    /// rather than incur the performance cost.
    ///
    /// If you need accurate signature counts with Redis storage, consider:
    /// - Using Redis SCAN manually in a background task
    /// - Maintaining a separate counter in Redis (requires coordination)
    /// - Using metrics/monitoring from Redis directly
    fn len(&self) -> usize {
        0
    }

    /// Check if storage is empty.
    ///
    /// # Limitation
    ///
    /// **This always returns false for RedisStorage.**
    ///
    /// Similar to `len()`, determining if Redis storage is truly empty would require
    /// a SCAN operation. We conservatively return `false` (assume not empty) to avoid
    /// the performance cost.
    ///
    /// This does not affect rate limiting functionality - it only impacts any code
    /// that relies on checking if the registry is empty, which is rare.
    fn is_empty(&self) -> bool {
        false
    }

    fn clear(&self) {
        // Use pattern matching to delete all keys with our prefix
        let pattern = format!("{}*", self.config.key_prefix);

        let clear_fn = async {
            let mut conn = self.connection.write().await;

            // Use SCAN to find keys and delete them
            let mut cursor = 0;
            loop {
                let (new_cursor, keys): (u64, Vec<String>) = redis::cmd("SCAN")
                    .arg(cursor)
                    .arg("MATCH")
                    .arg(&pattern)
                    .arg("COUNT")
                    .arg(100)
                    .query_async(&mut *conn)
                    .await
                    .unwrap_or((0, vec![]));

                if !keys.is_empty() {
                    let _: Result<(), RedisError> = conn.del(&keys).await;
                }

                if new_cursor == 0 {
                    break;
                }
                cursor = new_cursor;
            }
        };

        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            tokio::task::block_in_place(|| handle.block_on(clear_fn));
        } else {
            let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
            rt.block_on(clear_fn);
        }
    }

    fn for_each<F>(&self, mut f: F)
    where
        F: FnMut(&EventSignature, &EventState),
    {
        // Use SCAN to iterate over keys
        let pattern = format!("{}*", self.config.key_prefix);

        let for_each_fn = async {
            let mut conn = self.connection.write().await;
            let mut cursor = 0;

            loop {
                let (new_cursor, keys): (u64, Vec<String>) = redis::cmd("SCAN")
                    .arg(cursor)
                    .arg("MATCH")
                    .arg(&pattern)
                    .arg("COUNT")
                    .arg(100)
                    .query_async(&mut *conn)
                    .await
                    .unwrap_or((0, vec![]));

                for key in keys {
                    if let Some(sig_str) = key.strip_prefix(&self.config.key_prefix) {
                        if let Ok(sig_hash) = sig_str.parse::<u64>() {
                            let signature = EventSignature::from_hash(sig_hash);
                            if let Ok(Some(state)) = self.get(&signature).await {
                                f(&signature, &state);
                            }
                        }
                    }
                }

                if new_cursor == 0 {
                    break;
                }
                cursor = new_cursor;
            }
        };

        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            tokio::task::block_in_place(|| handle.block_on(for_each_fn));
        } else {
            let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
            rt.block_on(for_each_fn);
        }
    }

    fn retain<F>(&self, mut f: F)
    where
        F: FnMut(&EventSignature, &mut EventState) -> bool,
    {
        // Use SCAN to iterate and selectively delete
        let pattern = format!("{}*", self.config.key_prefix);

        let retain_fn = async {
            let mut conn = self.connection.write().await;
            let mut cursor = 0;

            loop {
                let (new_cursor, keys): (u64, Vec<String>) = redis::cmd("SCAN")
                    .arg(cursor)
                    .arg("MATCH")
                    .arg(&pattern)
                    .arg("COUNT")
                    .arg(100)
                    .query_async(&mut *conn)
                    .await
                    .unwrap_or((0, vec![]));

                for key in keys {
                    if let Some(sig_str) = key.strip_prefix(&self.config.key_prefix) {
                        if let Ok(sig_hash) = sig_str.parse::<u64>() {
                            let signature = EventSignature::from_hash(sig_hash);
                            if let Ok(Some(mut state)) = self.get(&signature).await {
                                if !f(&signature, &mut state) {
                                    // Delete key - predicate returned false
                                    if let Err(e) = conn.del::<_, ()>(&key).await {
                                        tracing::warn!(
                                            error = %e,
                                            key = %key,
                                            "Failed to delete key from Redis during retain"
                                        );
                                    }
                                } else {
                                    // Update key - predicate returned true
                                    if let Err(e) = self.set(&signature, &state).await {
                                        tracing::warn!(
                                            error = %e,
                                            signature = %signature.as_hash(),
                                            "Failed to update key in Redis during retain"
                                        );
                                    }
                                }
                            }
                        }
                    }
                }

                if new_cursor == 0 {
                    break;
                }
                cursor = new_cursor;
            }
        };

        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            tokio::task::block_in_place(|| handle.block_on(retain_fn));
        } else {
            let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
            rt.block_on(retain_fn);
        }
    }
}
