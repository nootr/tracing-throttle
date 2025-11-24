//! Ports (interfaces) for the application layer.
//!
//! In hexagonal architecture, ports define the interfaces that the application
//! layer needs. Infrastructure adapters implement these ports.

use std::fmt::Debug;
use std::hash::Hash;
use std::time::Instant;

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
