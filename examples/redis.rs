//! Example demonstrating Redis-backed storage for distributed rate limiting.
//!
//! This example shows how to use RedisStorage to share rate limiting state
//! across multiple application instances. This is useful for:
//!
//! - Microservices that need consistent rate limiting across replicas
//! - Distributed systems where logs should be throttled globally
//! - Horizontal scaling scenarios where local state isn't sufficient
//!
//! # Quick Start
//!
//! 1. Start Redis using the provided docker-compose file:
//!    ```bash
//!    cd examples/redis
//!    docker-compose up -d
//!    ```
//!
//! 2. Run the example (from project root):
//!    ```bash
//!    cargo run --example redis --features redis-storage
//!    ```
//!
//! 3. Stop Redis when done:
//!    ```bash
//!    cd examples/redis
//!    docker-compose down
//!    ```
//!
//! See `examples/redis/README.md` for detailed instructions and troubleshooting.
//!
//! # Alternative: Run Redis manually
//!
//! If you prefer not to use docker-compose:
//! ```bash
//! docker run -p 6379:6379 redis:7-alpine
//! ```
//!
//! # Testing Distributed Rate Limiting
//!
//! Run multiple instances in different terminals to see shared rate limiting:
//! ```bash
//! # Terminal 1
//! cargo run --example redis --features redis-storage
//!
//! # Terminal 2 (at the same time)
//! cargo run --example redis --features redis-storage
//! ```
//!
//! Both instances will share the same rate limit counters via Redis.
//! You'll notice that the total number of allowed events is split across both processes!

use std::sync::Arc;
use std::time::Duration;
use tracing::{error, info, warn};
use tracing_subscriber::prelude::*;
use tracing_throttle::{
    Policy, RedisStorage, RedisStorageConfig, SystemClock, TracingRateLimitLayer,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Configure Redis storage
    let redis_config = RedisStorageConfig {
        key_prefix: "tracing_throttle".to_string(),
        ttl: Duration::from_secs(300), // 5 minutes
    };

    // Create Redis storage backend
    let storage = RedisStorage::connect_with_config("redis://127.0.0.1:6379", redis_config).await?;

    // Configure rate limiting with token bucket policy
    // 50 burst capacity, 5 tokens/sec (300/min sustained)
    let policy = Policy::token_bucket(50.0, 5.0)?;
    let clock = Arc::new(SystemClock::new());
    let rate_limit = TracingRateLimitLayer::with_storage(storage, policy, clock);

    // Set up tracing subscriber
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_filter(rate_limit))
        .init();

    info!("Starting Redis-backed rate limiting example");
    info!("Redis storage allows multiple processes to share rate limits");
    info!("");

    // Simulate burst of identical logs (will be rate limited as a group)
    info!("=== Burst Test (identical logs) ===");
    for _ in 1..=100 {
        info!(message = "Burst event", "Repeated event");
    }

    tokio::time::sleep(Duration::from_millis(500)).await;

    // Simulate different log levels (each limited independently)
    info!("");
    info!("=== Multi-Level Test (different signatures) ===");
    for _ in 1..=30 {
        info!(event = "info", "Info level event");
        warn!(event = "warn", "Warning level event");
        error!(event = "error", "Error level event");
    }

    tokio::time::sleep(Duration::from_millis(500)).await;

    // Simulate logs with different fields (each creates unique signature)
    info!("");
    info!("=== Field Variation Test ===");
    for user_id in 1..=20 {
        info!(user_id = user_id, "User action logged");
        info!(user_id = user_id, "User action logged"); // Duplicate
        info!(user_id = user_id, "User action logged"); // Duplicate
    }

    tokio::time::sleep(Duration::from_millis(500)).await;

    // Demonstrate token refill over time
    info!("");
    info!("=== Token Refill Test ===");
    info!("Waiting 2 seconds for token refill (10 tokens at 5/sec)...");
    tokio::time::sleep(Duration::from_secs(2)).await;

    for _ in 1..=15 {
        info!("After refill");
    }

    tokio::time::sleep(Duration::from_secs(1)).await;

    // Final summary
    info!("");
    info!("=== Example Complete ===");
    info!("Rate limiting state is persisted in Redis");
    info!("Try running this example again immediately to see shared state");

    // Wait a bit for suppression summaries to be emitted
    tokio::time::sleep(Duration::from_secs(2)).await;

    Ok(())
}
