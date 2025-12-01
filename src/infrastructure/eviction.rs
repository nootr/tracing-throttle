//! Eviction strategies for signature management.
//!
//! This module provides different strategies for evicting signatures when
//! storage limits are reached, including priority-based and memory-based eviction.

use std::sync::Arc;

/// Function type for calculating signature priority.
///
/// Takes a signature and its state, returns a priority value.
/// Higher priority = kept longer during eviction.
/// Lower priority = evicted first.
pub type PriorityFn<K, V> = Arc<dyn Fn(&K, &V) -> u32 + Send + Sync>;

/// Eviction strategy for managing signature storage.
///
/// Determines which signatures should be evicted when storage limits are reached.
#[derive(Clone)]
pub enum EvictionStrategy<K, V>
where
    K: Clone,
{
    /// LRU (Least Recently Used) eviction with approximate sampling.
    ///
    /// This is the default strategy. When eviction is needed, samples a few
    /// entries and evicts the one accessed longest ago.
    Lru,

    /// Priority-based eviction using a custom function.
    ///
    /// When eviction is needed, samples entries and evicts the one with
    /// the lowest priority value returned by the function.
    ///
    /// # Example: Prioritize by log level
    ///
    /// ```ignore
    /// use std::sync::Arc;
    /// use tracing_throttle::EvictionStrategy;
    ///
    /// let strategy = EvictionStrategy::Priority(Arc::new(|_sig, state| {
    ///     // Keep ERROR events longer than INFO events
    ///     match state.metadata.as_ref().map(|m| m.level.as_str()) {
    ///         Some("ERROR") => 100,
    ///         Some("WARN") => 50,
    ///         Some("INFO") => 10,
    ///         Some("DEBUG") => 1,
    ///         _ => 5
    ///     }
    /// }));
    /// ```
    Priority(PriorityFn<K, V>),

    /// Memory-based eviction with byte limit.
    ///
    /// Evicts signatures (using LRU) when total memory usage exceeds the limit.
    /// Memory is estimated based on struct sizes and string lengths.
    ///
    /// # Memory Estimation
    ///
    /// Memory includes:
    /// - Key size (EventSignature ~= 32 bytes)
    /// - Value size (EventState ~= 40-80 bytes)
    /// - Metadata strings if present (~50-200 bytes)
    ///
    /// Note: Estimates are conservative and may not match exact heap usage.
    Memory {
        /// Maximum memory usage in bytes
        max_bytes: usize,
    },

    /// Combined priority and memory limits.
    ///
    /// Uses priority function for eviction decisions, but also enforces
    /// a maximum memory limit. Evicts lowest priority when either limit is exceeded.
    PriorityWithMemory {
        /// Priority calculation function
        priority_fn: PriorityFn<K, V>,
        /// Maximum memory usage in bytes
        max_bytes: usize,
    },
}

impl<K, V> EvictionStrategy<K, V>
where
    K: Clone,
{
    /// Check if this strategy requires memory tracking.
    pub fn tracks_memory(&self) -> bool {
        matches!(
            self,
            EvictionStrategy::Memory { .. } | EvictionStrategy::PriorityWithMemory { .. }
        )
    }

    /// Get the memory limit if configured.
    pub fn memory_limit(&self) -> Option<usize> {
        match self {
            EvictionStrategy::Memory { max_bytes } => Some(*max_bytes),
            EvictionStrategy::PriorityWithMemory { max_bytes, .. } => Some(*max_bytes),
            _ => None,
        }
    }

    /// Check if this strategy uses a priority function.
    pub fn uses_priority(&self) -> bool {
        matches!(
            self,
            EvictionStrategy::Priority(_) | EvictionStrategy::PriorityWithMemory { .. }
        )
    }
}

// Manual Debug implementation since PriorityFn doesn't implement Debug
impl<K, V> std::fmt::Debug for EvictionStrategy<K, V>
where
    K: Clone,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EvictionStrategy::Lru => write!(f, "EvictionStrategy::Lru"),
            EvictionStrategy::Priority(_) => write!(f, "EvictionStrategy::Priority(<fn>)"),
            EvictionStrategy::Memory { max_bytes } => f
                .debug_struct("EvictionStrategy::Memory")
                .field("max_bytes", max_bytes)
                .finish(),
            EvictionStrategy::PriorityWithMemory {
                max_bytes,
                priority_fn: _,
            } => f
                .debug_struct("EvictionStrategy::PriorityWithMemory")
                .field("max_bytes", max_bytes)
                .field("priority_fn", &"<fn>")
                .finish(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tracks_memory() {
        let lru: EvictionStrategy<String, String> = EvictionStrategy::Lru;
        assert!(!lru.tracks_memory());

        let priority: EvictionStrategy<String, String> =
            EvictionStrategy::Priority(Arc::new(|_, _| 1));
        assert!(!priority.tracks_memory());

        let memory: EvictionStrategy<String, String> = EvictionStrategy::Memory { max_bytes: 1000 };
        assert!(memory.tracks_memory());

        let combined: EvictionStrategy<String, String> = EvictionStrategy::PriorityWithMemory {
            priority_fn: Arc::new(|_, _| 1),
            max_bytes: 1000,
        };
        assert!(combined.tracks_memory());
    }

    #[test]
    fn test_memory_limit() {
        let lru: EvictionStrategy<String, String> = EvictionStrategy::Lru;
        assert_eq!(lru.memory_limit(), None);

        let memory: EvictionStrategy<String, String> = EvictionStrategy::Memory { max_bytes: 5000 };
        assert_eq!(memory.memory_limit(), Some(5000));

        let combined: EvictionStrategy<String, String> = EvictionStrategy::PriorityWithMemory {
            priority_fn: Arc::new(|_, _| 1),
            max_bytes: 10000,
        };
        assert_eq!(combined.memory_limit(), Some(10000));
    }

    #[test]
    fn test_uses_priority() {
        let lru: EvictionStrategy<String, String> = EvictionStrategy::Lru;
        assert!(!lru.uses_priority());

        let priority: EvictionStrategy<String, String> =
            EvictionStrategy::Priority(Arc::new(|_, _| 1));
        assert!(priority.uses_priority());

        let memory: EvictionStrategy<String, String> = EvictionStrategy::Memory { max_bytes: 1000 };
        assert!(!memory.uses_priority());

        let combined: EvictionStrategy<String, String> = EvictionStrategy::PriorityWithMemory {
            priority_fn: Arc::new(|_, _| 1),
            max_bytes: 1000,
        };
        assert!(combined.uses_priority());
    }
}
