//! Mock implementations for testing.
//!
//! This module provides test doubles for infrastructure adapters,
//! enabling controlled testing of application logic.

pub mod clock;
pub mod layer;

pub use clock::MockClock;
pub use layer::MockCaptureLayer;
