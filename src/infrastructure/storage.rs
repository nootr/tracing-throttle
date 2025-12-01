//! Storage implementations for event state.
//!
//! Provides concurrent, sharded storage for tracking event suppression state.

use crate::application::metrics::Metrics;
use crate::application::ports::Storage;
use crate::infrastructure::eviction::EvictionStrategy;
use dashmap::DashMap;
use std::hash::Hash;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;

/// Internal wrapper that tracks last access time for LRU eviction.
#[derive(Debug)]
struct StorageEntry<V> {
    value: V,
    /// Timestamp as nanoseconds since a reference point for atomic updates
    last_access_nanos: AtomicU64,
}

impl<V> StorageEntry<V> {
    /// Create a new storage entry with the given value and initial access time.
    ///
    /// # Overflow Handling
    ///
    /// If the duration from epoch exceeds u64::MAX nanoseconds (~584 years),
    /// it saturates at u64::MAX.
    fn new(value: V, now: Instant, epoch: Instant) -> Self {
        let nanos = now
            .saturating_duration_since(epoch)
            .as_nanos()
            .min(u64::MAX as u128) as u64;
        Self {
            value,
            last_access_nanos: AtomicU64::new(nanos),
        }
    }

    /// Update the last access time.
    ///
    /// Uses Release ordering to ensure the timestamp update is visible to other threads.
    fn update_access(&self, now: Instant, epoch: Instant) {
        let nanos = now
            .saturating_duration_since(epoch)
            .as_nanos()
            .min(u64::MAX as u128) as u64;
        // Use Release ordering to ensure visibility of timestamp updates
        self.last_access_nanos.store(nanos, Ordering::Release);
    }

    /// Get the last access time.
    ///
    /// Uses Acquire ordering to synchronize with Release stores.
    ///
    /// # Overflow Handling
    ///
    /// If adding the stored duration to epoch would overflow (practically impossible),
    /// returns the epoch. This means extremely old entries will appear to have been
    /// accessed at epoch time, making them candidates for LRU eviction.
    fn last_access(&self, epoch: Instant) -> Instant {
        // Use Acquire ordering to synchronize with Release store
        let nanos = self.last_access_nanos.load(Ordering::Acquire);
        epoch
            .checked_add(std::time::Duration::from_nanos(nanos))
            .unwrap_or(epoch)
    }
}

/// Thread-safe sharded storage backed by DashMap with configurable eviction.
///
/// DashMap provides lock-free reads and fine-grained locking for writes,
/// making it ideal for high-throughput logging scenarios.
///
/// Supports multiple eviction strategies:
/// - LRU: Evict least recently used (default)
/// - Priority: Evict lowest priority entries
/// - Memory: Evict when memory limit exceeded
pub struct ShardedStorage<K, V>
where
    K: Eq + Hash + Clone,
{
    map: DashMap<K, StorageEntry<V>>,
    max_entries: Option<usize>,
    /// Reference point for tracking timestamps
    epoch: Instant,
    /// Optional metrics for tracking evictions
    metrics: Option<Metrics>,
    /// Eviction strategy
    eviction_strategy: Option<EvictionStrategy<K, V>>,
    /// Current memory usage in bytes (when memory tracking enabled)
    current_memory_bytes: AtomicUsize,
}

// Manual Debug implementation since EvictionStrategy contains function pointers
impl<K, V> std::fmt::Debug for ShardedStorage<K, V>
where
    K: Eq + Hash + Clone + std::fmt::Debug,
    V: std::fmt::Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShardedStorage")
            .field("map", &format!("{} entries", self.map.len()))
            .field("max_entries", &self.max_entries)
            .field("eviction_strategy", &self.eviction_strategy)
            .field(
                "current_memory_bytes",
                &self.current_memory_bytes.load(Ordering::Relaxed),
            )
            .finish()
    }
}

impl<K, V> ShardedStorage<K, V>
where
    K: Eq + Hash + Clone,
{
    /// Create a new sharded storage instance with no size limit.
    pub fn new() -> Self {
        Self {
            map: DashMap::new(),
            max_entries: None,
            epoch: Instant::now(),
            metrics: None,
            eviction_strategy: None,
            current_memory_bytes: AtomicUsize::new(0),
        }
    }

    /// Create a new sharded storage instance with a maximum entry limit.
    ///
    /// When the limit is reached, approximate LRU eviction is used to make space.
    pub fn with_max_entries(max_entries: usize) -> Self {
        Self {
            map: DashMap::new(),
            max_entries: Some(max_entries),
            epoch: Instant::now(),
            metrics: None,
            eviction_strategy: None,
            current_memory_bytes: AtomicUsize::new(0),
        }
    }

    /// Set the metrics tracker for this storage.
    ///
    /// When metrics are set, eviction events will be recorded.
    pub fn with_metrics(mut self, metrics: Metrics) -> Self {
        self.metrics = Some(metrics);
        self
    }

    /// Set the eviction strategy for this storage.
    ///
    /// # Example: Priority-based eviction
    /// ```ignore
    /// use std::sync::Arc;
    /// use tracing_throttle::infrastructure::storage::ShardedStorage;
    /// use tracing_throttle::infrastructure::eviction::EvictionStrategy;
    ///
    /// let storage = ShardedStorage::new()
    ///     .with_eviction_strategy(EvictionStrategy::Priority(Arc::new(|_k, _v| 10)));
    /// ```
    pub fn with_eviction_strategy(mut self, strategy: EvictionStrategy<K, V>) -> Self {
        self.eviction_strategy = Some(strategy);
        self
    }

    /// Estimate memory usage for a key-value pair.
    ///
    /// This is an approximation used for memory-based eviction.
    fn estimate_entry_size(_key: &K, _value: &V) -> usize {
        // Conservative estimate of entry size
        // Includes: key, value, storage overhead, and estimated heap allocations
        let base_size = std::mem::size_of::<K>()
            + std::mem::size_of::<V>()
            + std::mem::size_of::<StorageEntry<V>>();

        // Add estimate for heap-allocated data (strings, collections, etc.)
        // This is a rough approximation - actual usage may vary
        let estimated_heap = 200; // Conservative estimate for metadata strings

        base_size + estimated_heap
    }

    /// Check if eviction is needed based on configured limits.
    fn should_evict(&self) -> bool {
        // Check entry count limit
        let entries_exceeded = self.max_entries.is_some_and(|max| self.len() >= max);

        // Check memory limit if strategy uses memory tracking
        let memory_exceeded = self
            .eviction_strategy
            .as_ref()
            .and_then(|s| s.memory_limit())
            .is_some_and(|max_bytes| {
                self.current_memory_bytes.load(Ordering::Relaxed) >= max_bytes
            });

        entries_exceeded || memory_exceeded
    }

    /// Evict one entry using the configured strategy.
    ///
    /// Strategy determines which entry to evict:
    /// - LRU: Oldest accessed entry (default)
    /// - Priority: Lowest priority entry
    /// - Memory: LRU when memory limit exceeded
    fn evict_one(&self) {
        let key_to_evict = match &self.eviction_strategy {
            Some(EvictionStrategy::Priority(priority_fn))
            | Some(EvictionStrategy::PriorityWithMemory { priority_fn, .. }) => {
                self.find_lowest_priority(priority_fn)
            }
            Some(EvictionStrategy::Memory { .. }) | Some(EvictionStrategy::Lru) | None => {
                self.find_oldest_lru()
            }
        };

        // Evict the selected entry
        if let Some(key) = key_to_evict {
            if let Some((_, entry)) = self.map.remove(&key) {
                // Update memory tracking if enabled
                if self
                    .eviction_strategy
                    .as_ref()
                    .is_some_and(|s| s.tracks_memory())
                {
                    let size = Self::estimate_entry_size(&key, &entry.value);
                    self.current_memory_bytes.fetch_sub(size, Ordering::Relaxed);
                }

                // Record eviction in metrics if available
                if let Some(ref metrics) = self.metrics {
                    metrics.record_eviction();
                }
            }
        }
    }

    /// Find the entry with the oldest access time (LRU).
    fn find_oldest_lru(&self) -> Option<K> {
        const SAMPLE_SIZE: usize = 5;

        let mut oldest_key: Option<K> = None;
        let mut oldest_time = Instant::now();

        for (idx, entry) in self.map.iter().enumerate() {
            if idx >= SAMPLE_SIZE {
                break;
            }

            let access_time = entry.value().last_access(self.epoch);
            if oldest_key.is_none() || access_time < oldest_time {
                oldest_time = access_time;
                oldest_key = Some(entry.key().clone());
            }
        }

        oldest_key
    }

    /// Find the entry with the lowest priority.
    fn find_lowest_priority(
        &self,
        priority_fn: &crate::infrastructure::eviction::PriorityFn<K, V>,
    ) -> Option<K> {
        const SAMPLE_SIZE: usize = 20; // Larger sample for better priority selection

        let mut lowest_key: Option<K> = None;
        let mut lowest_priority = u32::MAX;

        for (idx, entry) in self.map.iter().enumerate() {
            if idx >= SAMPLE_SIZE {
                break;
            }

            let priority = priority_fn(entry.key(), &entry.value().value);
            if lowest_key.is_none() || priority < lowest_priority {
                lowest_priority = priority;
                lowest_key = Some(entry.key().clone());
            }
        }

        lowest_key
    }

    /// Get the number of entries.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Check if the storage is empty.
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Clear all entries.
    pub fn clear(&self) {
        self.map.clear();
    }
}

impl<K, V> Default for ShardedStorage<K, V>
where
    K: Eq + Hash + Clone,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<K, V> Clone for ShardedStorage<K, V>
where
    K: Eq + Hash + Clone,
    V: Clone,
{
    fn clone(&self) -> Self {
        let new_storage = Self {
            map: DashMap::new(),
            max_entries: self.max_entries,
            epoch: self.epoch,
            metrics: self.metrics.clone(),
            eviction_strategy: self.eviction_strategy.clone(),
            current_memory_bytes: AtomicUsize::new(
                self.current_memory_bytes.load(Ordering::Relaxed),
            ),
        };
        for entry in self.map.iter() {
            let key = entry.key().clone();
            let storage_entry = StorageEntry {
                value: entry.value().value.clone(),
                last_access_nanos: AtomicU64::new(
                    entry.value().last_access_nanos.load(Ordering::Relaxed),
                ),
            };
            new_storage.map.insert(key, storage_entry);
        }
        new_storage
    }
}

// Implement the Storage port
impl<K, V> Storage<K, V> for ShardedStorage<K, V>
where
    K: Hash + Eq + Clone + Send + Sync + std::fmt::Debug,
    V: Send + Sync + std::fmt::Debug,
{
    fn with_entry_mut<F, R>(&self, key: K, factory: impl FnOnce() -> V, accessor: F) -> R
    where
        F: FnOnce(&mut V) -> R,
    {
        let is_new_entry = !self.map.contains_key(&key);

        // Check if we need to make space before inserting
        if is_new_entry && self.should_evict() {
            self.evict_one();
        }

        let now = Instant::now();
        let epoch = self.epoch;

        let entry = self.map.entry(key.clone());
        let mut storage_entry = entry.or_insert_with(|| {
            let value = factory();

            // Track memory for new entries if enabled
            if is_new_entry
                && self
                    .eviction_strategy
                    .as_ref()
                    .is_some_and(|s| s.tracks_memory())
            {
                let size = Self::estimate_entry_size(&key, &value);
                self.current_memory_bytes.fetch_add(size, Ordering::Relaxed);
            }

            StorageEntry::new(value, now, epoch)
        });

        // Update access time
        storage_entry.update_access(now, epoch);

        // Provide mutable access to the wrapped value
        accessor(&mut storage_entry.value)
    }

    fn len(&self) -> usize {
        self.map.len()
    }

    fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    fn clear(&self) {
        self.map.clear()
    }

    fn for_each<F>(&self, mut f: F)
    where
        F: FnMut(&K, &V),
    {
        for entry in self.map.iter() {
            f(entry.key(), &entry.value().value);
        }
    }

    fn retain<F>(&self, mut f: F)
    where
        F: FnMut(&K, &mut V) -> bool,
    {
        self.map
            .retain(|k, storage_entry| f(k, &mut storage_entry.value));
    }
}

// Implement Storage for Arc<ShardedStorage> to allow it to be used directly
impl<K, V> Storage<K, V> for std::sync::Arc<ShardedStorage<K, V>>
where
    K: Hash + Eq + Clone + Send + Sync + std::fmt::Debug,
    V: Send + Sync + std::fmt::Debug,
{
    fn with_entry_mut<F, R>(&self, key: K, factory: impl FnOnce() -> V, accessor: F) -> R
    where
        F: FnOnce(&mut V) -> R,
    {
        (**self).with_entry_mut(key, factory, accessor)
    }

    fn len(&self) -> usize {
        (**self).len()
    }

    fn is_empty(&self) -> bool {
        (**self).is_empty()
    }

    fn clear(&self) {
        (**self).clear()
    }

    fn for_each<F>(&self, f: F)
    where
        F: FnMut(&K, &V),
    {
        (**self).for_each(f)
    }

    fn retain<F>(&self, f: F)
    where
        F: FnMut(&K, &mut V) -> bool,
    {
        (**self).retain(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::ports::Storage as StorageTrait;

    #[test]
    fn test_basic_operations() {
        let storage = ShardedStorage::<String, i32>::new();

        // Use the Storage trait methods
        storage.with_entry_mut("key1".to_string(), || 100, |_v| {});
        storage.with_entry_mut("key2".to_string(), || 200, |_v| {});

        assert_eq!(storage.len(), 2);
        assert!(!storage.is_empty());
    }

    #[test]
    fn test_clear() {
        let storage = ShardedStorage::<String, i32>::new();

        storage.with_entry_mut("key1".to_string(), || 100, |_v| {});
        storage.with_entry_mut("key2".to_string(), || 200, |_v| {});
        assert_eq!(storage.len(), 2);

        storage.clear();
        assert_eq!(storage.len(), 0);
        assert!(storage.is_empty());
    }

    #[test]
    fn test_concurrent_access() {
        use std::sync::Arc;
        use std::thread;

        let storage = Arc::new(ShardedStorage::<String, i32>::new());
        let mut handles = vec![];

        for i in 0..10 {
            let storage_clone = Arc::clone(&storage);
            let handle = thread::spawn(move || {
                for j in 0..100 {
                    let key = format!("key_{}_{}", i, j);
                    let value = i * 100 + j;
                    storage_clone.with_entry_mut(key, || value, |_v| {});
                }
            });
            handles.push(handle);
        }

        for handle in handles {
            handle.join().unwrap();
        }

        assert_eq!(storage.len(), 1000);
    }

    #[test]
    fn test_lru_eviction() {
        let storage = ShardedStorage::<String, i32>::with_max_entries(5);

        // Insert 5 entries - should all fit
        for i in 0..5 {
            storage.with_entry_mut(format!("key{}", i), || i, |_v| {});
        }
        assert_eq!(storage.len(), 5);

        // Insert one more - should evict one
        storage.with_entry_mut("key5".to_string(), || 5, |_v| {});
        assert_eq!(storage.len(), 5);

        // Continue inserting - size should stay at 5
        for i in 6..10 {
            storage.with_entry_mut(format!("key{}", i), || i, |_v| {});
        }
        assert_eq!(storage.len(), 5);
    }

    #[test]
    fn test_lru_access_order() {
        let storage = ShardedStorage::<String, i32>::with_max_entries(3);

        // Insert 3 entries
        storage.with_entry_mut("key0".to_string(), || 0, |_v| {});
        storage.with_entry_mut("key1".to_string(), || 1, |_v| {});
        storage.with_entry_mut("key2".to_string(), || 2, |_v| {});

        // Sleep a bit to ensure time difference
        std::thread::sleep(std::time::Duration::from_millis(10));

        // Access key0 to update its access time
        storage.with_entry_mut("key0".to_string(), || 0, |_v| {});

        std::thread::sleep(std::time::Duration::from_millis(10));

        // Insert a new entry - should evict one of the older unaccessed entries
        storage.with_entry_mut("key3".to_string(), || 3, |_v| {});

        // key0 should still exist since we accessed it recently
        let mut key0_exists = false;
        storage.with_entry_mut(
            "key0".to_string(),
            || 999,
            |v| {
                if *v == 0 {
                    key0_exists = true;
                }
            },
        );

        assert!(key0_exists, "key0 should not have been evicted");
        assert_eq!(storage.len(), 3);
    }

    #[test]
    fn test_for_each() {
        let storage = ShardedStorage::<String, i32>::new();

        storage.with_entry_mut("a".to_string(), || 1, |_v| {});
        storage.with_entry_mut("b".to_string(), || 2, |_v| {});
        storage.with_entry_mut("c".to_string(), || 3, |_v| {});

        let mut sum = 0;
        storage.for_each(|_k, v| {
            sum += v;
        });

        assert_eq!(sum, 6);
    }

    #[test]
    fn test_retain() {
        let storage = ShardedStorage::<String, i32>::new();

        storage.with_entry_mut("a".to_string(), || 1, |_v| {});
        storage.with_entry_mut("b".to_string(), || 2, |_v| {});
        storage.with_entry_mut("c".to_string(), || 3, |_v| {});

        // Retain only values > 1
        storage.retain(|_k, v| *v > 1);

        assert_eq!(storage.len(), 2);
    }
}
