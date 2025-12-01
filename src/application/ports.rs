//! Ports (interfaces) for the application layer.
//!
//! In hexagonal architecture, ports define the interfaces that the application
//! layer needs. Infrastructure adapters implement these ports.

use std::fmt::Debug;
use std::hash::Hash;
use std::time::Instant;

/// Candidate entry for eviction consideration.
///
/// Contains all information needed to make eviction decisions.
/// Values are cloned to avoid lifetime issues with concurrent maps.
pub struct EvictionCandidate<K, V> {
    /// The key of the entry
    pub key: K,
    /// The value of the entry (cloned)
    pub value: V,
    /// Last access time for LRU-based strategies
    pub last_access: Instant,
}

/// Port for eviction policy decisions.
///
/// This abstraction allows the storage layer to delegate eviction decisions
/// to pluggable policies. Infrastructure provides concrete implementations
/// (LruEviction, PriorityEviction, MemoryBasedEviction).
///
/// The API uses owned values (cloned) to avoid lifetime issues with
/// concurrent map guards. This is acceptable because eviction is infrequent
/// and values are typically small.
pub trait EvictionPolicy<K, V>: Send + Sync + Debug
where
    K: Clone,
    V: Clone,
{
    /// Select a victim from the given candidates for eviction.
    ///
    /// # Arguments
    /// * `candidates` - Slice of candidates to evaluate (includes owned values)
    ///
    /// # Returns
    /// The key of the entry to evict, or None if no eviction should occur
    fn select_victim(&self, candidates: &[EvictionCandidate<K, V>]) -> Option<K>;

    /// Check if eviction should be triggered based on current state.
    ///
    /// # Arguments
    /// * `current_entries` - Current number of entries in storage
    /// * `current_memory_bytes` - Current memory usage in bytes (if tracked)
    ///
    /// # Returns
    /// True if eviction should occur
    fn should_evict(&self, current_entries: usize, current_memory_bytes: usize) -> bool;

    /// Check if this policy requires memory tracking.
    fn tracks_memory(&self) -> bool {
        false
    }

    /// Estimate memory size of a key-value pair.
    ///
    /// Only called if `tracks_memory()` returns true.
    fn estimate_entry_size(&self, _key: &K, _value: &V) -> usize {
        0
    }
}

/// Port for obtaining current time.
///
/// This abstraction allows the application layer to work with time
/// without depending on system clock implementation details.
/// Infrastructure provides concrete implementations (SystemClock, MockClock).
pub trait Clock: Send + Sync + Debug {
    /// Get the current instant.
    fn now(&self) -> Instant;
}

/// Port for concurrent key-value storage.
///
/// This abstraction allows the application layer to store and retrieve values
/// without depending on specific concurrent data structure implementations.
/// Infrastructure provides concrete implementations (ShardedStorage).
pub trait Storage<K, V>: Send + Sync + Debug
where
    K: Hash + Eq + Clone + Send + Sync,
    V: Send + Sync,
{
    /// Access an entry with mutable access, creating it if necessary.
    ///
    /// # Arguments
    /// * `key` - The key to look up
    /// * `factory` - Function to create a new value if the key doesn't exist
    /// * `accessor` - Function that gets mutable access to the value
    ///
    /// # Returns
    /// The result from the accessor function
    fn with_entry_mut<F, R>(&self, key: K, factory: impl FnOnce() -> V, accessor: F) -> R
    where
        F: FnOnce(&mut V) -> R;

    /// Get the number of entries in the storage.
    fn len(&self) -> usize;

    /// Check if the storage is empty.
    fn is_empty(&self) -> bool;

    /// Clear all entries from the storage.
    fn clear(&self);

    /// Iterate over all entries, providing access to both key and value.
    fn for_each<F>(&self, f: F)
    where
        F: FnMut(&K, &V);

    /// Remove entries for which the predicate returns false.
    fn retain<F>(&self, f: F)
    where
        F: FnMut(&K, &mut V) -> bool;
}
