# tracing-throttle Best Practices

A practical guide for using `tracing-throttle` effectively in your applications.

## Understanding Event Signatures

`tracing-throttle` deduplicates log events based on their **signature**, which by default consists of:
- Log level (INFO, WARN, ERROR, etc.)
- Target (module path)
- Message text

**Important**: Field values are NOT included in signatures by default.

## Best Practice #1: Use Static Message Strings

### ❌ Don't: Put Variable Data in Message Strings

```rust
// WRONG: Each message has a unique string, creating unique signatures
for i in 0..100 {
    info!("Processing request #{}", i);  // 100 different signatures!
}

// WRONG: Dynamic error messages
error!("Failed to connect to {}", server_url);  // Every URL is unique
```

**Problem**: Every log creates a unique signature, so throttling never kicks in.

### ✅ Do: Use Structured Fields for Variable Data

```rust
// CORRECT: Message is constant, variable data in fields
for i in 0..100 {
    info!(request_num = i, "Processing request");  // 1 signature, throttled after limit
}

// CORRECT: Static message with structured fields
error!(server = %server_url, "Failed to connect to server");
```

**Result**: All logs share the same signature and get throttled as a group.

## Best Practice #2: Choose the Right Throttling Policy

### Token Bucket (Recommended Default)

Best for: General-purpose rate limiting with burst tolerance.

```rust
// Allow 10 events/second with burst capacity of 5
let policy = Policy::token_bucket(10.0, 5.0).unwrap();
```

**Use when**: You want to allow occasional bursts but maintain an average rate.

### Time-Window

Best for: Strict limits per time period.

```rust
// Allow exactly 5 events per 1-second window
let policy = Policy::time_window(5, Duration::from_secs(1)).unwrap();
```

**Use when**: You need predictable limits (e.g., "no more than X logs per minute").

### Count-Based

Best for: Limiting total occurrences.

```rust
// Allow only 3 occurrences, then suppress all remaining
let policy = Policy::count_based(3).unwrap();
```

**Use when**: You want to see a few examples then suppress (e.g., startup warnings).

### Exponential Backoff

Best for: Progressively reducing log frequency.

```rust
// Emit at: 1st, 2nd, 4th, 8th, 16th, 32nd...
let policy = Policy::exponential_backoff();
```

**Use when**: You want to see that an issue is ongoing without flooding logs.

## Best Practice #3: Per-Entity Throttling

Sometimes you want to throttle per-user, per-endpoint, or per-resource instead of globally.

### When to Use

```rust
// Throttle per user - each user_id gets independent rate limits
let layer = TracingRateLimitLayer::builder()
    .with_event_fields(vec!["user_id".to_string()])
    .with_policy(Policy::time_window(10, Duration::from_secs(1)).unwrap())
    .build()
    .unwrap();

info!(user_id = 123, "API request");  // User 123's quota
info!(user_id = 456, "API request");  // User 456's separate quota
```

### Common Use Cases

```rust
// Per-endpoint rate limiting
.with_event_fields(vec!["endpoint".to_string()])
warn!(endpoint = "/api/users", "High latency detected");

// Per-service rate limiting in microservices
.with_event_fields(vec!["service".to_string()])
error!(service = "auth-service", "Connection timeout");

// Per-resource monitoring
.with_event_fields(vec!["resource_id".to_string()])
warn!(resource_id = "db-primary", "Connection pool exhausted");
```

### Warning: High Cardinality

Be careful with high-cardinality fields (request IDs, timestamps, UUIDs):

```rust
// ❌ DON'T: Creates a unique signature for every request
.with_event_fields(vec!["request_id".to_string()])
info!(request_id = uuid, "Request processed");  // No throttling!

// ✅ DO: Use low-cardinality fields
.with_event_fields(vec!["user_id".to_string()])
info!(user_id = 123, request_id = uuid, "Request processed");
```

## Best Practice #4: Memory Management

### Default Settings

- Tracks up to 10,000 unique signatures
- ~200-400 bytes per signature
- ~2-4 MB typical memory usage

### Adjust Based on Your Scale

```rust
// Small application with few unique log signatures
let layer = TracingRateLimitLayer::builder()
    .with_max_signatures(1_000)
    .build()
    .unwrap();

// Large application with many unique log patterns
let layer = TracingRateLimitLayer::builder()
    .with_max_signatures(50_000)
    .build()
    .unwrap();

// High-cardinality (per-user throttling with millions of users)
let layer = TracingRateLimitLayer::builder()
    .with_max_signatures(100_000)
    .with_eviction_strategy(EvictionStrategy::lru())
    .build()
    .unwrap();
```

## Best Practice #5: Monitor Your Throttling

Always monitor metrics to ensure throttling is working as expected.

```rust
let layer = TracingRateLimitLayer::builder()
    .with_policy(Policy::time_window(100, Duration::from_secs(1)).unwrap())
    .with_summary_interval(Duration::from_secs(60))
    .build()
    .unwrap();

let metrics = layer.metrics().clone();

// In a monitoring task
tokio::spawn(async move {
    loop {
        tokio::time::sleep(Duration::from_secs(60)).await;

        let allowed = metrics.events_allowed();
        let suppressed = metrics.events_suppressed();
        let total = allowed + suppressed;

        if total > 0 {
            let suppression_rate = (suppressed as f64 / total as f64) * 100.0;
            info!(
                events_allowed = allowed,
                events_suppressed = suppressed,
                suppression_rate = format!("{:.1}%", suppression_rate),
                "Throttling metrics"
            );
        }
    }
});
```

## Testing Your Throttling Configuration

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_throttling_behavior() {
        let layer = TracingRateLimitLayer::builder()
            .with_policy(Policy::count_based(5).unwrap())
            .build()
            .unwrap();

        let metrics = layer.metrics().clone();

        let subscriber = tracing_subscriber::registry()
            .with(layer);

        tracing::subscriber::with_default(subscriber, || {
            // Generate 20 identical log messages
            for _ in 0..20 {
                info!("Test message");
            }
        });

        // Verify only 5 were allowed, 15 suppressed
        assert_eq!(metrics.events_allowed(), 5);
        assert_eq!(metrics.events_suppressed(), 15);
    }
}
```

## Key Takeaways

1. **Keep message strings static** - put variable data in structured fields
2. **Choose the right policy** for your use case (token bucket is a good default)
3. **Use `.with_event_fields()`** for per-entity throttling (users, endpoints, services)
4. **Avoid high-cardinality fields** in signatures (UUIDs, timestamps, request IDs)
5. **Monitor metrics** to verify throttling is working correctly
6. **Adjust `max_signatures`** based on your application's log diversity
7. **Test your configuration** to ensure expected throttling behavior

## When NOT to Use tracing-throttle

- **Low-volume applications**: If you log < 100 events/second, throttling overhead may not be worth it
- **Already deduplicated**: If your log aggregation system deduplicates, you might be duplicating effort
- **Critical debug sessions**: Disable temporarily when debugging specific issues
- **Log everything requirements**: Some compliance scenarios require complete log retention

## Further Reading

- [Official Documentation](https://docs.rs/tracing-throttle)
- [GitHub Repository](https://github.com/nootr/tracing-throttle)
- [tracing Ecosystem](https://github.com/tokio-rs/tracing)
