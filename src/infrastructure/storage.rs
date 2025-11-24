//! Storage implementations for event state.
//!
//! Provides concurrent, sharded storage for tracking event suppression state.

use crate::application::ports::Storage;
use dashmap::DashMap;
use std::hash::Hash;

/// Thread-safe sharded storage backed by DashMap.
///
/// DashMap provides lock-free reads and fine-grained locking for writes,
/// making it ideal for high-throughput logging scenarios.
#[derive(Debug)]
pub struct ShardedStorage<K, V>
where
    K: Eq + Hash + Clone,
{
    map: DashMap<K, V>,
}

impl<K, V> ShardedStorage<K, V>
where
    K: Eq + Hash + Clone,
{
    /// Create a new sharded storage instance.
    pub fn new() -> Self {
        Self {
            map: DashMap::new(),
        }
    }

    /// Insert or update a value.
    pub fn insert(&self, key: K, value: V) {
        self.map.insert(key, value);
    }

    /// Get a reference to a value.
    pub fn get<Q>(&self, key: &Q) -> Option<dashmap::mapref::one::Ref<'_, K, V>>
    where
        K: std::borrow::Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.map.get(key)
    }

    /// Get a mutable reference to a value.
    pub fn get_mut<Q>(&self, key: &Q) -> Option<dashmap::mapref::one::RefMut<'_, K, V>>
    where
        K: std::borrow::Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.map.get_mut(key)
    }

    /// Check if a key exists.
    pub fn contains_key<Q>(&self, key: &Q) -> bool
    where
        K: std::borrow::Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.map.contains_key(key)
    }

    /// Remove a key and return its value.
    pub fn remove<Q>(&self, key: &Q) -> Option<(K, V)>
    where
        K: std::borrow::Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.map.remove(key)
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

    /// Get or insert a value using a closure.
    pub fn entry(&self, key: K) -> dashmap::mapref::entry::Entry<'_, K, V> {
        self.map.entry(key)
    }

    /// Iterate over all key-value pairs.
    pub fn iter(&self) -> dashmap::iter::Iter<'_, K, V> {
        self.map.iter()
    }

    /// Retain only the elements that satisfy the predicate.
    pub fn retain(&self, f: impl FnMut(&K, &mut V) -> bool) {
        self.map.retain(f);
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
        let new_storage = Self::new();
        for entry in self.map.iter() {
            new_storage.insert(entry.key().clone(), entry.value().clone());
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
        let entry = self.map.entry(key);
        let mut value_ref = entry.or_insert_with(factory);
        accessor(&mut value_ref)
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
            f(entry.key(), entry.value());
        }
    }

    fn retain<F>(&self, f: F)
    where
        F: FnMut(&K, &mut V) -> bool,
    {
        self.map.retain(f);
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

    #[test]
    fn test_basic_operations() {
        let storage = ShardedStorage::new();

        storage.insert("key1", 100);
        storage.insert("key2", 200);

        assert_eq!(*storage.get("key1").unwrap(), 100);
        assert_eq!(*storage.get("key2").unwrap(), 200);
        assert!(storage.get("key3").is_none());

        assert_eq!(storage.len(), 2);
        assert!(!storage.is_empty());
    }

    #[test]
    fn test_update() {
        let storage = ShardedStorage::new();

        storage.insert("key", 100);
        assert_eq!(*storage.get("key").unwrap(), 100);

        storage.insert("key", 200);
        assert_eq!(*storage.get("key").unwrap(), 200);
    }

    #[test]
    fn test_remove() {
        let storage = ShardedStorage::new();

        storage.insert("key", 100);
        assert!(storage.contains_key("key"));

        let removed = storage.remove("key");
        assert_eq!(removed, Some(("key", 100)));
        assert!(!storage.contains_key("key"));
    }

    #[test]
    fn test_clear() {
        let storage = ShardedStorage::new();

        storage.insert("key1", 100);
        storage.insert("key2", 200);
        assert_eq!(storage.len(), 2);

        storage.clear();
        assert_eq!(storage.len(), 0);
        assert!(storage.is_empty());
    }

    #[test]
    fn test_concurrent_access() {
        use std::sync::Arc;
        use std::thread;

        let storage = Arc::new(ShardedStorage::new());
        let mut handles = vec![];

        for i in 0..10 {
            let storage_clone = Arc::clone(&storage);
            let handle = thread::spawn(move || {
                for j in 0..100 {
                    storage_clone.insert(format!("key_{}_{}", i, j), i * 100 + j);
                }
            });
            handles.push(handle);
        }

        for handle in handles {
            handle.join().unwrap();
        }

        assert_eq!(storage.len(), 1000);
    }
}
