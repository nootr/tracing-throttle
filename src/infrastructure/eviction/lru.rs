//! LRU (Least Recently Used) eviction adapter.

use crate::application::ports::{EvictionCandidate, EvictionPolicy};

/// LRU eviction policy with entry count limit.
///
/// Evicts the least recently accessed entry when the limit is reached.
/// Uses approximate sampling for performance.
#[derive(Debug, Clone)]
pub struct LruEviction {
    /// Maximum number of entries before eviction
    max_entries: usize,
}

impl LruEviction {
    /// Create a new LRU eviction policy with the given entry limit.
    pub fn new(max_entries: usize) -> Self {
        Self { max_entries }
    }
}

impl<K, V> EvictionPolicy<K, V> for LruEviction
where
    K: Clone,
    V: Clone,
{
    fn select_victim(&self, candidates: &[EvictionCandidate<K, V>]) -> Option<K> {
        // Find the oldest accessed entry (LRU doesn't need value access)
        candidates
            .iter()
            .min_by_key(|candidate| candidate.last_access)
            .map(|candidate| candidate.key.clone())
    }

    fn should_evict(&self, current_entries: usize, _current_memory_bytes: usize) -> bool {
        current_entries >= self.max_entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn test_lru_select_oldest() {
        let policy = LruEviction::new(10);
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
                last_access: now - std::time::Duration::from_secs(10),
            },
            EvictionCandidate {
                key: "key3".to_string(),
                value: 300,
                last_access: now - std::time::Duration::from_secs(5),
            },
        ];

        let victim = policy.select_victim(&candidates);
        assert_eq!(victim, Some("key2".to_string()));
    }

    #[test]
    fn test_lru_should_evict() {
        use crate::application::ports::EvictionPolicy;
        let policy: LruEviction = LruEviction::new(100);

        assert!(!<LruEviction as EvictionPolicy<String, i32>>::should_evict(
            &policy, 99, 0
        ));
        assert!(<LruEviction as EvictionPolicy<String, i32>>::should_evict(
            &policy, 100, 0
        ));
        assert!(<LruEviction as EvictionPolicy<String, i32>>::should_evict(
            &policy, 101, 0
        ));
    }

    #[test]
    fn test_lru_no_memory_tracking() {
        use crate::application::ports::EvictionPolicy;
        let policy: LruEviction = LruEviction::new(100);
        assert!(!<LruEviction as EvictionPolicy<String, i32>>::tracks_memory(&policy));
    }
}
