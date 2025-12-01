//! Priority-based eviction adapter.

use crate::application::ports::{EvictionCandidate, EvictionPolicy};
use std::sync::Arc;

/// Function type for calculating entry priority.
///
/// Takes a key and value, returns a priority value.
/// Higher priority = kept longer during eviction.
/// Lower priority = evicted first.
pub type PriorityFn<K, V> = Arc<dyn Fn(&K, &V) -> u32 + Send + Sync>;

/// Priority-based eviction policy.
///
/// Evicts entries with the lowest priority value returned by the function.
#[derive(Clone)]
pub struct PriorityEviction<K, V>
where
    K: Clone,
{
    /// Maximum number of entries before eviction
    max_entries: usize,
    /// Priority calculation function
    priority_fn: PriorityFn<K, V>,
}

impl<K, V> PriorityEviction<K, V>
where
    K: Clone,
{
    /// Create a new priority-based eviction policy.
    ///
    /// # Arguments
    /// * `max_entries` - Maximum number of entries before eviction
    /// * `priority_fn` - Function to calculate priority for each entry
    ///
    /// # Example
    /// ```ignore
    /// use std::sync::Arc;
    /// use tracing_throttle::infrastructure::eviction::PriorityEviction;
    ///
    /// let policy = PriorityEviction::new(5000, Arc::new(|_key, state| {
    ///     match state.metadata.as_ref().map(|m| m.level.as_str()) {
    ///         Some("ERROR") => 100,
    ///         Some("WARN") => 50,
    ///         Some("INFO") => 10,
    ///         _ => 5,
    ///     }
    /// }));
    /// ```
    pub fn new(max_entries: usize, priority_fn: PriorityFn<K, V>) -> Self {
        Self {
            max_entries,
            priority_fn,
        }
    }
}

impl<K, V> EvictionPolicy<K, V> for PriorityEviction<K, V>
where
    K: Clone,
    V: Clone,
{
    fn select_victim(&self, candidates: &[EvictionCandidate<K, V>]) -> Option<K> {
        // Find the entry with the lowest priority
        candidates
            .iter()
            .map(|candidate| {
                let priority = (self.priority_fn)(&candidate.key, &candidate.value);
                (candidate, priority)
            })
            .min_by_key(|(_, priority)| *priority)
            .map(|(candidate, _)| candidate.key.clone())
    }

    fn should_evict(&self, current_entries: usize, _current_memory_bytes: usize) -> bool {
        current_entries >= self.max_entries
    }
}

// Manual Debug implementation since PriorityFn doesn't implement Debug
impl<K, V> std::fmt::Debug for PriorityEviction<K, V>
where
    K: Clone,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PriorityEviction")
            .field("max_entries", &self.max_entries)
            .field("priority_fn", &"<fn>")
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn test_priority_select_lowest() {
        let priority_fn = Arc::new(|_key: &String, value: &i32| *value as u32);
        let policy = PriorityEviction::new(10, priority_fn);
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
        assert_eq!(victim, Some("key2".to_string())); // Lowest priority value
    }

    #[test]
    fn test_priority_should_evict() {
        let priority_fn = Arc::new(|_key: &String, value: &i32| *value as u32);
        let policy = PriorityEviction::new(100, priority_fn);

        assert!(!policy.should_evict(99, 0));
        assert!(policy.should_evict(100, 0));
        assert!(policy.should_evict(101, 0));
    }

    #[test]
    fn test_priority_no_memory_tracking() {
        let priority_fn = Arc::new(|_key: &String, value: &i32| *value as u32);
        let policy = PriorityEviction::new(100, priority_fn);
        assert!(!policy.tracks_memory());
    }
}
