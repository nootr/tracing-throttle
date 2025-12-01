//! Domain layer - pure business logic with no external dependencies.
//!
//! This layer contains the core concepts and invariants of the rate limiting system:
//! - Event signature computation
//! - Rate limiting policies
//! - Suppression counters and summaries
//! - Event metadata for human-readable summaries
//!
//! All types in this layer are pure and easily testable.

pub mod metadata;
pub mod policy;
pub mod signature;
pub mod summary;
