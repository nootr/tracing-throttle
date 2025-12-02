//! Memory-based eviction adapter.

use crate::application::ports::{EvictionCandidate, EvictionPolicy};

/// Memory-based eviction policy with byte limit.
///
/// Evicts entries (using LRU) when total memory usage exceeds the limit.
/// Memory is estimated based on struct sizes and string lengths.
#[derive(Debug, Clone)]
pub struct MemoryEviction {
    /// Maximum memory usage in bytes
    max_bytes: usize,
}

impl MemoryEviction {
    /// Create a new memory-based eviction policy.
    ///
    /// # Arguments
    /// * `max_bytes` - Maximum memory usage in bytes
    ///
    /// # Example
    /// ```ignore
    /// use tracing_throttle::infrastructure::eviction::MemoryEviction;
    ///
    /// let policy = MemoryEviction::new(5 * 1024 * 1024); // 5MB limit
    /// ```
    pub fn new(max_bytes: usize) -> Self {
        Self { max_bytes }
    }

    /// Get the memory limit.
    pub fn max_bytes(&self) -> usize {
        self.max_bytes
    }
}

impl<K, V> EvictionPolicy<K, V> for MemoryEviction
where
    K: Clone,
    V: Clone,
{
    fn select_victim(&self, candidates: &[EvictionCandidate<K, V>]) -> Option<K> {
        // Use LRU strategy for memory-based eviction (doesn't need value access)
        candidates
            .iter()
            .min_by_key(|candidate| candidate.last_access)
            .map(|candidate| candidate.key.clone())
    }

    fn should_evict(&self, _current_entries: usize, current_memory_bytes: usize) -> bool {
        current_memory_bytes >= self.max_bytes
    }

    fn tracks_memory(&self) -> bool {
        true
    }

    fn estimate_entry_size(&self, _key: &K, _value: &V) -> usize {
        // Conservative estimate of entry size
        // Includes: key, value, storage overhead, and estimated heap allocations
        let base_size = std::mem::size_of::<K>() + std::mem::size_of::<V>() + 64; // Storage overhead

        // Add estimate for heap-allocated data (strings, collections, etc.)
        let estimated_heap = 200;

        base_size + estimated_heap
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn test_memory_select_lru() {
        let policy = MemoryEviction::new(1000);
        let now = Instant::now();

        let candidates = vec![
            EvictionCandidate {
                key: "key1".to_string(),
                value: 100,
                last_access: now,
            },
            EvictionCandidate {
                key: "key2".to_string(),
                value: 200,
                last_access: now
                    .checked_sub(std::time::Duration::from_secs(10))
                    .unwrap_or(now),
            },
            EvictionCandidate {
                key: "key3".to_string(),
                value: 300,
                last_access: now
                    .checked_sub(std::time::Duration::from_secs(5))
                    .unwrap_or(now),
            },
        ];

        let victim = policy.select_victim(&candidates);
        assert_eq!(victim, Some("key2".to_string())); // Oldest access
    }

    #[test]
    fn test_memory_should_evict() {
        use crate::application::ports::EvictionPolicy;
        let policy = MemoryEviction::new(1000);

        assert!(!<MemoryEviction as EvictionPolicy<String, i32>>::should_evict(&policy, 0, 999));
        assert!(<MemoryEviction as EvictionPolicy<String, i32>>::should_evict(&policy, 0, 1000));
        assert!(<MemoryEviction as EvictionPolicy<String, i32>>::should_evict(&policy, 0, 1001));
    }

    #[test]
    fn test_memory_tracks_memory() {
        use crate::application::ports::EvictionPolicy;
        let policy = MemoryEviction::new(1000);
        assert!(<MemoryEviction as EvictionPolicy<String, i32>>::tracks_memory(&policy));
    }

    #[test]
    fn test_memory_estimate_entry_size() {
        use crate::application::ports::EvictionPolicy;
        let policy = MemoryEviction::new(1000);
        let key = "test_key";
        let value = 42;

        let size = <MemoryEviction as EvictionPolicy<&str, i32>>::estimate_entry_size(
            &policy, &key, &value,
        );
        assert!(size > 0);
    }
}
