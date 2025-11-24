//! Infrastructure layer - external adapters and integrations.
//!
//! This layer provides adapters for:
//! - Clock abstraction (system time vs mock)
//! - Storage implementations (sharded maps)
//! - Tracing integration (Layer trait)

pub mod clock;
pub mod layer;
pub mod storage;
