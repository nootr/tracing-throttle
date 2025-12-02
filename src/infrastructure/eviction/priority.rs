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

    #[test]
    fn test_priority_empty_candidates() {
        let priority_fn = Arc::new(|_key: &String, value: &i32| *value as u32);
        let policy = PriorityEviction::new(10, priority_fn);
        let candidates: Vec<EvictionCandidate<String, i32>> = vec![];

        let victim = policy.select_victim(&candidates);
        assert_eq!(victim, None, "Should return None for empty candidate list");
    }

    #[test]
    fn test_priority_single_candidate() {
        let priority_fn = Arc::new(|_key: &String, value: &i32| *value as u32);
        let policy = PriorityEviction::new(10, priority_fn);
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
    fn test_priority_all_same_priority() {
        let priority_fn = Arc::new(|_key: &String, _value: &i32| 100u32); // All have same priority
        let policy = PriorityEviction::new(10, priority_fn);
        let now = Instant::now();

        let candidates = vec![
            EvictionCandidate {
                key: "key1".to_string(),
                value: 10,
                last_access: now,
            },
            EvictionCandidate {
                key: "key2".to_string(),
                value: 20,
                last_access: now,
            },
            EvictionCandidate {
                key: "key3".to_string(),
                value: 30,
                last_access: now,
            },
        ];

        let victim = policy.select_victim(&candidates);
        assert!(
            victim.is_some(),
            "Should select a victim even when all have equal priority"
        );
    }

    #[test]
    fn test_priority_u32_max_priority() {
        let priority_fn = Arc::new(
            |_key: &String, value: &i32| {
                if *value == 100 {
                    u32::MAX
                } else {
                    1
                }
            },
        );
        let policy = PriorityEviction::new(10, priority_fn);
        let now = Instant::now();

        let candidates = vec![
            EvictionCandidate {
                key: "max_priority".to_string(),
                value: 100,
                last_access: now,
            },
            EvictionCandidate {
                key: "low_priority".to_string(),
                value: 50,
                last_access: now,
            },
        ];

        let victim = policy.select_victim(&candidates);
        assert_eq!(
            victim,
            Some("low_priority".to_string()),
            "Should evict low priority even when another has u32::MAX"
        );
    }

    #[test]
    fn test_priority_zero_priority() {
        let priority_fn = Arc::new(
            |_key: &String, value: &i32| {
                if *value == 0 {
                    0
                } else {
                    100
                }
            },
        );
        let policy = PriorityEviction::new(10, priority_fn);
        let now = Instant::now();

        let candidates = vec![
            EvictionCandidate {
                key: "zero_priority".to_string(),
                value: 0,
                last_access: now,
            },
            EvictionCandidate {
                key: "high_priority".to_string(),
                value: 100,
                last_access: now,
            },
        ];

        let victim = policy.select_victim(&candidates);
        assert_eq!(
            victim,
            Some("zero_priority".to_string()),
            "Should evict entry with zero priority"
        );
    }

    #[test]
    fn test_priority_many_candidates() {
        let priority_fn = Arc::new(|_key: &String, value: &i32| *value as u32);
        let policy = PriorityEviction::new(1000, priority_fn);
        let now = Instant::now();

        // Create 100 candidates with varying priorities
        let candidates: Vec<_> = (0..100)
            .map(|i| EvictionCandidate {
                key: format!("key{}", i),
                value: i,
                last_access: now,
            })
            .collect();

        let victim = policy.select_victim(&candidates);
        assert_eq!(
            victim,
            Some("key0".to_string()),
            "Should select entry with lowest priority (0)"
        );
    }

    #[test]
    fn test_priority_complex_priority_function() {
        // Priority based on multiple factors
        let priority_fn = Arc::new(|key: &String, value: &i32| {
            let key_bonus = if key.starts_with("important") {
                1000
            } else {
                0
            };
            let value_priority = (*value as u32) * 10;
            key_bonus + value_priority
        });
        let policy = PriorityEviction::new(10, priority_fn);
        let now = Instant::now();

        let candidates = vec![
            EvictionCandidate {
                key: "important_1".to_string(),
                value: 1,
                last_access: now,
            },
            EvictionCandidate {
                key: "normal_100".to_string(),
                value: 100,
                last_access: now,
            },
            EvictionCandidate {
                key: "normal_5".to_string(),
                value: 5,
                last_access: now,
            },
        ];

        let victim = policy.select_victim(&candidates);
        // normal_5 has priority 50, normal_100 has priority 1000, important_1 has priority 1010
        assert_eq!(
            victim,
            Some("normal_5".to_string()),
            "Should select entry with lowest computed priority"
        );
    }

    #[test]
    fn test_priority_with_negative_values() {
        let priority_fn = Arc::new(|_key: &String, value: &i32| value.unsigned_abs());
        let policy = PriorityEviction::new(10, priority_fn);
        let now = Instant::now();

        let candidates = vec![
            EvictionCandidate {
                key: "key_neg100".to_string(),
                value: -100,
                last_access: now,
            },
            EvictionCandidate {
                key: "key_pos10".to_string(),
                value: 10,
                last_access: now,
            },
            EvictionCandidate {
                key: "key_neg5".to_string(),
                value: -5,
                last_access: now,
            },
        ];

        let victim = policy.select_victim(&candidates);
        assert_eq!(
            victim,
            Some("key_neg5".to_string()),
            "Should handle negative values correctly"
        );
    }

    #[test]
    fn test_priority_zero_max_entries() {
        let priority_fn = Arc::new(|_key: &String, value: &i32| *value as u32);
        let policy = PriorityEviction::new(0, priority_fn);

        // should_evict should return true immediately
        assert!(policy.should_evict(0, 0));
        assert!(policy.should_evict(1, 0));
    }

    #[test]
    fn test_priority_large_max_entries() {
        let priority_fn = Arc::new(|_key: &String, value: &i32| *value as u32);
        let policy = PriorityEviction::new(usize::MAX, priority_fn);

        // Should not evict until reaching usize::MAX
        assert!(!policy.should_evict(0, 0));
        assert!(!policy.should_evict(1_000_000, 0));
        assert!(policy.should_evict(usize::MAX, 0));
    }

    #[test]
    fn test_priority_ignores_last_access_time() {
        let priority_fn = Arc::new(|_key: &String, value: &i32| *value as u32);
        let policy = PriorityEviction::new(10, priority_fn);
        let now = Instant::now();

        let old_time = now
            .checked_sub(std::time::Duration::from_secs(3600))
            .unwrap_or(now);

        let candidates = vec![
            EvictionCandidate {
                key: "old_high_priority".to_string(),
                value: 100,
                last_access: old_time, // 1 hour ago (or earlier)
            },
            EvictionCandidate {
                key: "new_low_priority".to_string(),
                value: 10,
                last_access: now, // Just accessed
            },
        ];

        let victim = policy.select_victim(&candidates);
        assert_eq!(
            victim,
            Some("new_low_priority".to_string()),
            "Priority should be based on priority_fn, not access time"
        );
    }
}
