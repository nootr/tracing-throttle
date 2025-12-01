//! Infrastructure layer - external adapters and integrations.
//!
//! This layer provides adapters for:
//! - Clock abstraction (system time vs mock)
//! - Storage implementations (sharded maps)
//! - Tracing integration (Layer trait)

pub mod clock;
pub mod layer;
pub mod storage;
pub(crate) mod visitor;

#[cfg(feature = "redis-storage")]
pub mod redis_storage;

/// Mock implementations for testing.
///
/// This module is only available when the `test-helpers` feature is enabled,
/// or during test builds. It provides controllable test doubles for testing
/// rate limiting behavior.
///
/// To use these mocks in integration tests, add to your `Cargo.toml`:
/// ```toml
/// [dev-dependencies]
/// tracing-throttle = { version = "*", features = ["test-helpers"] }
/// ```
#[cfg(any(test, feature = "test-helpers"))]
pub mod mocks;
