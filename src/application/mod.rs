//! Application layer - orchestration of domain logic.
//!
//! This layer coordinates the domain logic and manages the runtime behavior:
//! - Suppression registry (storage of event states)
//! - Rate limiter (decision making)
//! - Summary emitter (periodic summaries)
//!
//! ## Ports
//!
//! The application layer defines ports (traits) that infrastructure
//! adapters must implement. This keeps the application layer independent
//! from infrastructure details.

pub mod circuit_breaker;
pub mod emitter;
pub mod limiter;
pub mod metrics;
pub mod ports;
pub mod registry;
