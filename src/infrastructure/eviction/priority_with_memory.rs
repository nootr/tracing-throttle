//! Combined priority and memory-based eviction adapter.

use crate::application::ports::{EvictionCandidate, EvictionPolicy};
use crate::infrastructure::eviction::priority::PriorityFn;

/// Combined priority and memory-based eviction policy.
///
/// Uses priority function for eviction decisions, but also enforces
/// a maximum memory limit. Evicts lowest priority when either limit is exceeded.
#[derive(Clone)]
pub struct PriorityWithMemoryEviction<K, V>
where
    K: Clone,
{
    /// Maximum number of entries before eviction
    max_entries: usize,
    /// Priority calculation function
    priority_fn: PriorityFn<K, V>,
    /// Maximum memory usage in bytes
    max_bytes: usize,
}

impl<K, V> PriorityWithMemoryEviction<K, V>
where
    K: Clone,
{
    /// Create a new combined priority and memory eviction policy.
    ///
    /// # Arguments
    /// * `max_entries` - Maximum number of entries before eviction
    /// * `priority_fn` - Function to calculate priority for each entry
    /// * `max_bytes` - Maximum memory usage in bytes
    ///
    /// # Example
    /// ```ignore
    /// use std::sync::Arc;
    /// use tracing_throttle::infrastructure::eviction::PriorityWithMemoryEviction;
    ///
    /// let policy = PriorityWithMemoryEviction::new(
    ///     5000,
    ///     Arc::new(|_key, state| {
    ///         match state.metadata.as_ref().map(|m| m.level.as_str()) {
    ///             Some("ERROR") => 100,
    ///             _ => 10,
    ///         }
    ///     }),
    ///     10 * 1024 * 1024 // 10MB
    /// );
    /// ```
    pub fn new(max_entries: usize, priority_fn: PriorityFn<K, V>, max_bytes: usize) -> Self {
        Self {
            max_entries,
            priority_fn,
            max_bytes,
        }
    }

    /// Get the memory limit.
    pub fn max_bytes(&self) -> usize {
        self.max_bytes
    }
}

impl<K, V> EvictionPolicy<K, V> for PriorityWithMemoryEviction<K, V>
where
    K: Clone,
    V: Clone,
{
    fn select_victim(&self, candidates: &[EvictionCandidate<K, V>]) -> Option<K> {
        // Use priority-based selection
        candidates
            .iter()
            .map(|candidate| {
                let priority = (self.priority_fn)(&candidate.key, &candidate.value);
                (candidate, priority)
            })
            .min_by_key(|(_, priority)| *priority)
            .map(|(candidate, _)| candidate.key.clone())
    }

    fn should_evict(&self, current_entries: usize, current_memory_bytes: usize) -> bool {
        current_entries >= self.max_entries || current_memory_bytes >= self.max_bytes
    }

    fn tracks_memory(&self) -> bool {
        true
    }

    fn estimate_entry_size(&self, _key: &K, _value: &V) -> usize {
        // Conservative estimate matching MemoryEviction
        let base_size = std::mem::size_of::<K>() + std::mem::size_of::<V>() + 64; // Storage overhead

        let estimated_heap = 200;
        base_size + estimated_heap
    }
}

// Manual Debug implementation since PriorityFn doesn't implement Debug
impl<K, V> std::fmt::Debug for PriorityWithMemoryEviction<K, V>
where
    K: Clone,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PriorityWithMemoryEviction")
            .field("max_entries", &self.max_entries)
            .field("priority_fn", &"<fn>")
            .field("max_bytes", &self.max_bytes)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Instant;

    #[test]
    fn test_priority_with_memory_select_lowest_priority() {
        let priority_fn = Arc::new(|_key: &String, value: &i32| *value as u32);
        let policy = PriorityWithMemoryEviction::new(10, priority_fn, 1000);
        let now = Instant::now();

        let candidates = vec![
            EvictionCandidate {
                key: "key1".to_string(),
                value: 100,
                last_access: now,
            },
            EvictionCandidate {
                key: "key2".to_string(),
                value: 50,
                last_access: now,
            },
            EvictionCandidate {
                key: "key3".to_string(),
                value: 200,
                last_access: now,
            },
        ];

        let victim = policy.select_victim(&candidates);
        assert_eq!(victim, Some("key2".to_string())); // Lowest priority
    }

    #[test]
    fn test_priority_with_memory_should_evict_by_entries() {
        let priority_fn = Arc::new(|_key: &String, value: &i32| *value as u32);
        let policy = PriorityWithMemoryEviction::new(100, priority_fn, 10000);

        assert!(!policy.should_evict(99, 500));
        assert!(policy.should_evict(100, 500));
        assert!(policy.should_evict(101, 500));
    }

    #[test]
    fn test_priority_with_memory_should_evict_by_memory() {
        let priority_fn = Arc::new(|_key: &String, value: &i32| *value as u32);
        let policy = PriorityWithMemoryEviction::new(100, priority_fn, 1000);

        assert!(!policy.should_evict(50, 999));
        assert!(policy.should_evict(50, 1000));
        assert!(policy.should_evict(50, 1001));
    }

    #[test]
    fn test_priority_with_memory_tracks_memory() {
        let priority_fn = Arc::new(|_key: &String, value: &i32| *value as u32);
        let policy = PriorityWithMemoryEviction::new(100, priority_fn, 1000);
        assert!(policy.tracks_memory());
    }
}
