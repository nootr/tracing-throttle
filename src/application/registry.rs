//! Central registry for tracking event suppression state.
//!
//! The registry maintains state for each unique event signature, including
//! its rate limiting policy and suppression counters.

use crate::application::ports::{Clock, Storage};
use crate::domain::{policy::Policy, signature::EventSignature, summary::SuppressionCounter};
use std::sync::Arc;
use std::time::Instant;

/// State tracked for each event signature.
#[derive(Debug)]
pub struct EventState {
    /// Rate limiting policy for this event
    pub policy: Policy,
    /// Counter tracking suppressions
    pub counter: SuppressionCounter,
}

impl EventState {
    /// Create new event state with a policy.
    pub fn new(policy: Policy, initial_timestamp: Instant) -> Self {
        Self {
            policy,
            counter: SuppressionCounter::new(initial_timestamp),
        }
    }
}

/// Registry managing all event suppression state.
///
/// Uses the Storage port for high-performance concurrent access.
///
/// This type is generic over the storage implementation, allowing different
/// storage backends to be used. In production, use `Arc<ShardedStorage>`.
#[derive(Clone)]
pub struct SuppressionRegistry<S>
where
    S: Storage<EventSignature, EventState> + Clone,
{
    storage: S,
    clock: Arc<dyn Clock>,
    default_policy: Policy,
}

impl<S> SuppressionRegistry<S>
where
    S: Storage<EventSignature, EventState> + Clone,
{
    /// Create a new registry with storage, clock, and a default policy.
    ///
    /// All events will use the default policy unless overridden.
    pub fn new(storage: S, clock: Arc<dyn Clock>, default_policy: Policy) -> Self {
        Self {
            storage,
            clock,
            default_policy,
        }
    }

    /// Access or create event state for a signature with a callback.
    ///
    /// If this is the first time seeing this signature, creates new state
    /// with the default policy. The callback receives the event state and
    /// the current timestamp.
    pub fn with_event_state<F, R>(&self, signature: EventSignature, f: F) -> R
    where
        F: FnOnce(&mut EventState, Instant) -> R,
    {
        let now = self.clock.now();
        let default_policy = self.default_policy.clone();
        self.storage.with_entry_mut(
            signature,
            || EventState::new(default_policy, now),
            |state| f(state, now),
        )
    }

    /// Get the default policy.
    pub fn default_policy(&self) -> &Policy {
        &self.default_policy
    }

    /// Create a clone of the default policy.
    pub fn clone_default_policy(&self) -> Policy {
        self.default_policy.clone()
    }

    /// Get the number of tracked signatures.
    pub fn len(&self) -> usize {
        self.storage.len()
    }

    /// Check if the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.storage.is_empty()
    }

    /// Clear all tracked state.
    pub fn clear(&self) {
        self.storage.clear();
    }

    /// Iterate over all event states with a callback.
    pub fn for_each<F>(&self, f: F)
    where
        F: FnMut(&EventSignature, &EventState),
    {
        self.storage.for_each(f);
    }

    /// Remove expired or inactive signatures based on a predicate.
    pub fn cleanup<F>(&self, f: F)
    where
        F: FnMut(&EventSignature, &mut EventState) -> bool,
    {
        self.storage.retain(f);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::policy::Policy;
    use crate::infrastructure::clock::SystemClock;
    use crate::infrastructure::storage::ShardedStorage;
    use std::sync::Arc;

    #[test]
    fn test_registry_creation() {
        let storage = Arc::new(ShardedStorage::new());
        let clock = Arc::new(SystemClock::new());
        let policy = Policy::count_based(100);
        let registry = SuppressionRegistry::new(storage, clock, policy);

        assert_eq!(registry.len(), 0);
        assert!(registry.is_empty());
    }

    #[test]
    fn test_with_event_state() {
        let storage = Arc::new(ShardedStorage::new());
        let clock = Arc::new(SystemClock::new());
        let policy = Policy::count_based(100);
        let registry = SuppressionRegistry::new(storage, clock, policy);
        let sig = EventSignature::simple("INFO", "Test message");

        // First access creates state
        registry.with_event_state(sig, |state, _now| {
            assert_eq!(state.counter.count(), 1);
        });

        assert_eq!(registry.len(), 1);
        assert!(!registry.is_empty());

        // Second access retrieves existing state
        registry.with_event_state(sig, |state, now| {
            state.counter.record_suppression(now);
        });

        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn test_clear() {
        let storage = Arc::new(ShardedStorage::new());
        let clock = Arc::new(SystemClock::new());
        let policy = Policy::count_based(100);
        let registry = SuppressionRegistry::new(storage, clock, policy);

        for i in 0..10 {
            let sig = EventSignature::simple("INFO", &format!("Message {}", i));
            registry.with_event_state(sig, |_state, _now| {
                // State is created
            });
        }

        assert_eq!(registry.len(), 10);

        registry.clear();
        assert_eq!(registry.len(), 0);
        assert!(registry.is_empty());
    }

    #[test]
    fn test_concurrent_access() {
        use std::sync::Arc;
        use std::thread;

        let storage = Arc::new(ShardedStorage::new());
        let clock = Arc::new(SystemClock::new());
        let policy = Policy::count_based(100);
        let registry = Arc::new(SuppressionRegistry::new(storage, clock, policy));
        let mut handles = vec![];

        for i in 0..10 {
            let registry_clone = Arc::clone(&registry);
            let handle = thread::spawn(move || {
                for j in 0..100 {
                    let sig = EventSignature::simple("INFO", &format!("Msg_{}_{}", i, j));
                    registry_clone.with_event_state(sig, |_state, _now| {
                        // State is created
                    });
                }
            });
            handles.push(handle);
        }

        for handle in handles {
            handle.join().unwrap();
        }

        assert_eq!(registry.len(), 1000);
    }
}
