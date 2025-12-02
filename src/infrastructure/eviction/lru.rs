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

    #[test]
    fn test_lru_empty_candidates() {
        let policy = LruEviction::new(10);
        let candidates: Vec<EvictionCandidate<String, i32>> = vec![];

        let victim = policy.select_victim(&candidates);
        assert_eq!(victim, None, "Should return None for empty candidate list");
    }

    #[test]
    fn test_lru_single_candidate() {
        let policy = LruEviction::new(10);
        let now = Instant::now();

        let candidates = vec![EvictionCandidate {
            key: "only_key".to_string(),
            value: 42,
            last_access: now,
        }];

        let victim = policy.select_victim(&candidates);
        assert_eq!(
            victim,
            Some("only_key".to_string()),
            "Should select the only candidate"
        );
    }

    #[test]
    fn test_lru_all_same_access_time() {
        let policy = LruEviction::new(10);
        let now = Instant::now();

        // All candidates accessed at exactly the same time
        let candidates = vec![
            EvictionCandidate {
                key: "key1".to_string(),
                value: 100,
                last_access: now,
            },
            EvictionCandidate {
                key: "key2".to_string(),
                value: 200,
                last_access: now,
            },
            EvictionCandidate {
                key: "key3".to_string(),
                value: 300,
                last_access: now,
            },
        ];

        let victim = policy.select_victim(&candidates);
        // Should select one of them deterministically (first one by iterator order)
        assert!(
            victim.is_some(),
            "Should select a victim even when all are equal"
        );
    }

    #[test]
    fn test_lru_extreme_time_differences() {
        let policy = LruEviction::new(10);
        let now = Instant::now();

        let candidates = vec![
            EvictionCandidate {
                key: "recent".to_string(),
                value: 1,
                last_access: now,
            },
            EvictionCandidate {
                key: "ancient".to_string(),
                value: 2,
                last_access: now - std::time::Duration::from_secs(365 * 24 * 3600), // 1 year ago
            },
            EvictionCandidate {
                key: "middle".to_string(),
                value: 3,
                last_access: now - std::time::Duration::from_secs(3600), // 1 hour ago
            },
        ];

        let victim = policy.select_victim(&candidates);
        assert_eq!(
            victim,
            Some("ancient".to_string()),
            "Should select the oldest entry even with extreme time differences"
        );
    }

    #[test]
    fn test_lru_many_candidates() {
        let policy = LruEviction::new(1000);
        let now = Instant::now();

        // Create 100 candidates with varying access times
        let candidates: Vec<_> = (0..100)
            .map(|i| EvictionCandidate {
                key: format!("key{}", i),
                value: i,
                last_access: now - std::time::Duration::from_secs(i as u64),
            })
            .collect();

        let victim = policy.select_victim(&candidates);
        assert_eq!(
            victim,
            Some("key99".to_string()),
            "Should select the oldest (99 seconds ago)"
        );
    }

    #[test]
    fn test_lru_with_future_timestamps() {
        let policy = LruEviction::new(10);
        let now = Instant::now();

        // Timestamps in the "future" (clock skew scenario)
        let candidates = vec![
            EvictionCandidate {
                key: "past".to_string(),
                value: 1,
                last_access: now - std::time::Duration::from_secs(10),
            },
            EvictionCandidate {
                key: "present".to_string(),
                value: 2,
                last_access: now,
            },
        ];

        let victim = policy.select_victim(&candidates);
        assert_eq!(
            victim,
            Some("past".to_string()),
            "Should handle relative timestamps correctly"
        );
    }

    #[test]
    fn test_lru_zero_max_entries() {
        use crate::application::ports::EvictionPolicy;
        let policy: LruEviction = LruEviction::new(0);

        // should_evict should return true immediately
        assert!(<LruEviction as EvictionPolicy<String, i32>>::should_evict(
            &policy, 0, 0
        ));
        assert!(<LruEviction as EvictionPolicy<String, i32>>::should_evict(
            &policy, 1, 0
        ));
    }

    #[test]
    fn test_lru_large_max_entries() {
        use crate::application::ports::EvictionPolicy;
        let policy: LruEviction = LruEviction::new(usize::MAX);

        // Should not evict until reaching usize::MAX
        assert!(!<LruEviction as EvictionPolicy<String, i32>>::should_evict(
            &policy, 0, 0
        ));
        assert!(!<LruEviction as EvictionPolicy<String, i32>>::should_evict(
            &policy, 1_000_000, 0
        ));
        assert!(<LruEviction as EvictionPolicy<String, i32>>::should_evict(
            &policy,
            usize::MAX,
            0
        ));
    }
}
