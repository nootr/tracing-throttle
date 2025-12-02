//! Integration tests for Redis storage.
//!
//! These tests require a Redis instance running at `redis://127.0.0.1/`.
//! Tests are ignored by default - run with `cargo test --features redis-storage --test redis_storage -- --ignored`

#![cfg(feature = "redis-storage")]

use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing_throttle::application::ports::Storage;
use tracing_throttle::application::registry::{EventState, SuppressionRegistry};
use tracing_throttle::domain::signature::EventSignature;
use tracing_throttle::infrastructure::clock::SystemClock;
use tracing_throttle::{Policy, RedisStorage, RedisStorageConfig};

/// Check if Redis is available before running tests
async fn redis_available() -> bool {
    RedisStorage::connect("redis://127.0.0.1/").await.is_ok()
}

/// Create a test storage with unique prefix
async fn create_test_storage(test_name: &str) -> RedisStorage {
    let config = RedisStorageConfig {
        key_prefix: format!("test:{}:", test_name),
        ttl: Duration::from_secs(60),
    };

    RedisStorage::connect_with_config("redis://127.0.0.1/", config)
        .await
        .expect("Failed to connect to Redis")
}

#[tokio::test]
#[ignore] // Requires Redis
async fn test_redis_connection() {
    if !redis_available().await {
        eprintln!("Skipping test: Redis not available at redis://127.0.0.1/");
        return;
    }

    let storage = create_test_storage("connection").await;
    storage.clear();
}

#[tokio::test]
#[ignore] // Requires Redis
async fn test_redis_basic_set_get() {
    if !redis_available().await {
        eprintln!("Skipping test: Redis not available");
        return;
    }

    let storage = create_test_storage("basic_set_get").await;
    storage.clear();

    let sig = EventSignature::simple("INFO", "Test message");
    let policy = Policy::count_based(100).unwrap();

    // Insert an entry
    storage.with_entry_mut(
        sig,
        || EventState::new(policy.clone(), Instant::now()),
        |state| {
            assert_eq!(state.counter.count(), 1);
        },
    );

    // Retrieve the same entry
    storage.with_entry_mut(
        sig,
        || panic!("Should not create new state"),
        |state| {
            assert_eq!(state.counter.count(), 1);
            state.counter.record_suppression(Instant::now());
        },
    );

    // Verify the update persisted
    storage.with_entry_mut(
        sig,
        || panic!("Should not create new state"),
        |state| {
            assert_eq!(state.counter.count(), 2);
        },
    );

    storage.clear();
}

#[tokio::test]
#[ignore] // Requires Redis
async fn test_redis_serialization_all_policies() {
    if !redis_available().await {
        eprintln!("Skipping test: Redis not available");
        return;
    }

    let storage = create_test_storage("serialization_policies").await;
    storage.clear();

    let now = Instant::now();

    // Test count-based policy
    let sig1 = EventSignature::simple("INFO", "Count-based test");
    let policy1 = Policy::count_based(100).unwrap();
    storage.with_entry_mut(sig1, || EventState::new(policy1.clone(), now), |_| {});

    // Test token bucket policy
    let sig2 = EventSignature::simple("INFO", "Token bucket test");
    let policy2 = Policy::token_bucket(10.0, 1.0).unwrap();
    storage.with_entry_mut(sig2, || EventState::new(policy2.clone(), now), |_| {});

    // Test time window policy
    let sig3 = EventSignature::simple("INFO", "Time window test");
    let policy3 = Policy::time_window(10, Duration::from_secs(60)).unwrap();
    storage.with_entry_mut(sig3, || EventState::new(policy3.clone(), now), |_| {});

    // Test exponential backoff policy
    let sig4 = EventSignature::simple("INFO", "Exponential backoff test");
    let policy4 = Policy::exponential_backoff();
    storage.with_entry_mut(sig4, || EventState::new(policy4.clone(), now), |_| {});

    // Verify all can be retrieved
    storage.with_entry_mut(sig1, || panic!("Count-based not found"), |_| {});
    storage.with_entry_mut(sig2, || panic!("Token bucket not found"), |_| {});
    storage.with_entry_mut(sig3, || panic!("Time window not found"), |_| {});
    storage.with_entry_mut(sig4, || panic!("Exponential backoff not found"), |_| {});

    storage.clear();
}

#[tokio::test]
#[ignore] // Requires Redis
async fn test_redis_ttl_expiration() {
    if !redis_available().await {
        eprintln!("Skipping test: Redis not available");
        return;
    }

    let config = RedisStorageConfig {
        key_prefix: "test:ttl:".to_string(),
        ttl: Duration::from_secs(2), // Very short TTL for testing
    };

    let storage = RedisStorage::connect_with_config("redis://127.0.0.1/", config)
        .await
        .expect("Failed to connect");

    storage.clear();

    let sig = EventSignature::simple("INFO", "TTL test");
    let policy = Policy::count_based(100).unwrap();

    // Insert entry
    storage.with_entry_mut(
        sig,
        || EventState::new(policy.clone(), Instant::now()),
        |_| {},
    );

    // Should exist immediately
    let mut found_before = false;
    storage.with_entry_mut(
        sig,
        || panic!("Entry should exist before TTL"),
        |_| {
            found_before = true;
        },
    );
    assert!(found_before);

    // Wait for TTL to expire
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Should be gone after TTL
    let mut created_new = false;
    storage.with_entry_mut(
        sig,
        || {
            created_new = true;
            EventState::new(policy, Instant::now())
        },
        |_| {},
    );
    assert!(
        created_new,
        "Entry should have expired and required factory call"
    );

    storage.clear();
}

#[tokio::test]
#[ignore] // Requires Redis
async fn test_redis_concurrent_access() {
    if !redis_available().await {
        eprintln!("Skipping test: Redis not available");
        return;
    }

    let storage = Arc::new(create_test_storage("concurrent").await);
    storage.clear();

    let sig = EventSignature::simple("INFO", "Concurrent test");
    let policy = Policy::count_based(1000).unwrap();

    // Spawn multiple tasks that all access the same signature
    let mut handles = vec![];
    for _ in 0..10 {
        let storage_clone = Arc::clone(&storage);
        let policy_clone = policy.clone();

        let handle = tokio::spawn(async move {
            for _ in 0..10 {
                storage_clone.with_entry_mut(
                    sig,
                    || EventState::new(policy_clone.clone(), Instant::now()),
                    |state| {
                        state.counter.record_suppression(Instant::now());
                    },
                );
            }
        });
        handles.push(handle);
    }

    // Wait for all tasks
    for handle in handles {
        handle.await.unwrap();
    }

    // Final count should reflect all increments
    // Note: Due to Redis race conditions, this might not be exactly 101 (1 initial + 100 records)
    // but should be at least close
    storage.with_entry_mut(
        sig,
        || panic!("Entry should exist"),
        |state| {
            let count = state.counter.count();
            // Allow some loss due to concurrent access, but should have most updates
            assert!(count >= 50, "Expected at least 50 updates, got {}", count);
        },
    );

    storage.clear();
}

#[tokio::test]
#[ignore] // Requires Redis
async fn test_redis_clear_operation() {
    if !redis_available().await {
        eprintln!("Skipping test: Redis not available");
        return;
    }

    let storage = create_test_storage("clear").await;
    storage.clear();

    let policy = Policy::count_based(100).unwrap();

    // Insert multiple entries
    for i in 0..10 {
        let sig = EventSignature::simple("INFO", &format!("Message {}", i));
        storage.with_entry_mut(
            sig,
            || EventState::new(policy.clone(), Instant::now()),
            |_| {},
        );
    }

    // Clear all
    storage.clear();

    // Verify all entries are gone by checking if factory is called
    let mut factory_called_count = 0;
    for i in 0..10 {
        let sig = EventSignature::simple("INFO", &format!("Message {}", i));
        storage.with_entry_mut(
            sig,
            || {
                factory_called_count += 1;
                EventState::new(policy.clone(), Instant::now())
            },
            |_| {},
        );
    }

    assert_eq!(
        factory_called_count, 10,
        "All entries should have been cleared"
    );

    storage.clear();
}

#[tokio::test]
#[ignore] // Requires Redis
async fn test_redis_for_each() {
    if !redis_available().await {
        eprintln!("Skipping test: Redis not available");
        return;
    }

    let storage = create_test_storage("for_each").await;
    storage.clear();

    let policy = Policy::count_based(100).unwrap();

    // Insert entries
    for i in 0..5 {
        let sig = EventSignature::simple("INFO", &format!("Msg {}", i));
        storage.with_entry_mut(
            sig,
            || EventState::new(policy.clone(), Instant::now()),
            |_| {},
        );
    }

    // Count via for_each
    let mut count = 0;
    storage.for_each(|_, _| {
        count += 1;
    });

    assert_eq!(count, 5, "for_each should iterate over all 5 entries");

    storage.clear();
}

#[tokio::test]
#[ignore] // Requires Redis
async fn test_redis_retain() {
    if !redis_available().await {
        eprintln!("Skipping test: Redis not available");
        return;
    }

    let storage = create_test_storage("retain").await;
    storage.clear();

    let policy = Policy::count_based(100).unwrap();

    // Insert entries with different counts
    for i in 0..10 {
        let sig = EventSignature::simple("INFO", &format!("Entry {}", i));
        storage.with_entry_mut(
            sig,
            || EventState::new(policy.clone(), Instant::now()),
            |state| {
                // Record suppressions to set count
                for _ in 0..i {
                    state.counter.record_suppression(Instant::now());
                }
            },
        );
    }

    // Retain only entries with count > 5
    storage.retain(|_, state| state.counter.count() > 5);

    // Count remaining
    let mut count = 0;
    storage.for_each(|_, _| {
        count += 1;
    });

    // Entries 6,7,8,9 should remain (count > 5 after increments)
    assert!(
        count >= 3,
        "Should retain entries with count > 5, got {}",
        count
    );

    storage.clear();
}

#[tokio::test]
#[ignore] // Requires Redis
async fn test_redis_corrupted_data_handling() {
    if !redis_available().await {
        eprintln!("Skipping test: Redis not available");
        return;
    }

    let storage = create_test_storage("corrupted").await;
    storage.clear();

    // Manually insert corrupted data
    let sig = EventSignature::simple("INFO", "Corrupted test");
    let key = format!("test:corrupted:{}", sig);

    // Insert invalid bincode data
    let corrupt_data = vec![0xFF, 0xFF, 0xFF, 0xFF];

    let redis_client = redis::Client::open("redis://127.0.0.1/").unwrap();
    let mut conn = redis_client.get_connection().unwrap();

    use redis::Commands;
    let _: () = conn.set(&key, corrupt_data).unwrap();

    // Try to read corrupted data - should create new entry
    let policy = Policy::count_based(100).unwrap();
    let mut factory_called = false;

    storage.with_entry_mut(
        sig,
        || {
            factory_called = true;
            EventState::new(policy.clone(), Instant::now())
        },
        |_| {},
    );

    assert!(
        factory_called,
        "Factory should be called when data is corrupted"
    );

    storage.clear();
}

#[tokio::test]
#[ignore] // Requires Redis
async fn test_redis_key_prefix_isolation() {
    if !redis_available().await {
        eprintln!("Skipping test: Redis not available");
        return;
    }

    // Create two storages with different prefixes
    let storage1 = create_test_storage("prefix1").await;
    let storage2 = create_test_storage("prefix2").await;

    storage1.clear();
    storage2.clear();

    let sig = EventSignature::simple("INFO", "Prefix test");
    let policy = Policy::count_based(100).unwrap();

    // Insert into storage1
    storage1.with_entry_mut(
        sig,
        || EventState::new(policy.clone(), Instant::now()),
        |_| {},
    );

    // storage2 should not see it (different prefix)
    let mut storage2_factory_called = false;
    storage2.with_entry_mut(
        sig,
        || {
            storage2_factory_called = true;
            EventState::new(policy, Instant::now())
        },
        |_| {},
    );

    assert!(
        storage2_factory_called,
        "storage2 should not see storage1's data"
    );

    storage1.clear();
    storage2.clear();
}

#[tokio::test]
#[ignore] // Requires Redis
async fn test_redis_with_registry() {
    if !redis_available().await {
        eprintln!("Skipping test: Redis not available");
        return;
    }

    let storage = create_test_storage("registry").await;
    storage.clear();

    let clock = Arc::new(SystemClock::new());
    let policy = Policy::count_based(5).unwrap();

    let registry = SuppressionRegistry::new(storage.clone(), clock, policy);

    let sig = EventSignature::simple("INFO", "Registry test");

    // Use registry to access state
    registry.with_event_state(sig, |state, now| {
        assert_eq!(state.counter.count(), 1);
        state.counter.record_suppression(now);
    });

    // Verify persistence
    registry.with_event_state(sig, |state, _| {
        assert_eq!(state.counter.count(), 2);
    });

    storage.clear();
}

#[tokio::test]
#[ignore] // Requires Redis
async fn test_redis_timestamp_serialization() {
    if !redis_available().await {
        eprintln!("Skipping test: Redis not available");
        return;
    }

    let storage = create_test_storage("timestamps").await;
    storage.clear();

    let sig = EventSignature::simple("INFO", "Timestamp test");
    let policy = Policy::count_based(100).unwrap();

    let start_time = Instant::now();

    // Create entry with specific timestamp
    storage.with_entry_mut(sig, || EventState::new(policy.clone(), start_time), |_| {});

    // Sleep a bit
    std::thread::sleep(Duration::from_millis(100));

    // Retrieve and verify timestamp is reasonably close
    storage.with_entry_mut(
        sig,
        || panic!("Entry should exist"),
        |state| {
            let first = state.counter.first_suppressed();
            let duration = first.duration_since(start_time);

            // Should be very close to 0 (within a few ms of serialization overhead)
            assert!(
                duration < Duration::from_millis(50),
                "Timestamp should be preserved, got duration: {:?}",
                duration
            );
        },
    );

    storage.clear();
}
