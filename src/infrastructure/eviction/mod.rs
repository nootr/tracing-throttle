//! Eviction policy adapters for signature management.
//!
//! This module provides different adapters implementing the EvictionPolicy port,
//! including LRU, priority-based, memory-based, and combined eviction strategies.
//!
//! In hexagonal architecture, these are adapters (infrastructure layer)
//! that implement the EvictionPolicy port (application layer).

pub mod lru;
pub mod memory;
pub mod priority;
pub mod priority_with_memory;

pub use lru::LruEviction;
pub use memory::MemoryEviction;
pub use priority::{PriorityEviction, PriorityFn};
pub use priority_with_memory::PriorityWithMemoryEviction;
