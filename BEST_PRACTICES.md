# tracing-throttle Best Practices

A practical guide for using `tracing-throttle` effectively in production.

## Understanding Event Signatures

`tracing-throttle` deduplicates events based on their **signature**, which by default consists of:

- **Event level** (INFO, WARN, ERROR, etc.)
- **Target** (module path)
- **Message text**
- **ALL field values** (since v0.4.0)

This means events with different field values are treated as **semantically different** and NOT deduplicated:

```rust
error!(user_id = 123, "Failed to fetch user");  // Signature A
error!(user_id = 456, "Failed to fetch user");  // Signature B - DIFFERENT!
```

Both errors are logged because they represent failures for different users. This prevents accidental loss of important context.

## Best Practice #1: Keep Message Strings Static

### ❌ Don't: Embed Variable Data in Messages

```rust
// WRONG: Each message creates a unique signature
for user_id in 0..100 {
    error!("Failed to fetch user {}", user_id);  // 100 different messages!
}
```

**Problem**: Message text is part of the signature. Every variation creates a unique signature, preventing throttling.

### ✅ Do: Use Structured Fields for Variable Data

```rust
// CORRECT: Static message, variable data in fields
for user_id in 0..100 {
    error!(user_id = user_id, "Failed to fetch user");
}
```

**Result**: All errors share the same message but have different `user_id` values, so each user's errors are tracked independently.

## Best Practice #2: Understand Field-Based Throttling

Since all field values are included in signatures by default, identical field values are throttled together:

```rust
// Same user_id, same message = same signature
for _ in 0..1000 {
    error!(user_id = 123, "Failed to fetch user");
}
// With default policy: First 50 logged immediately, then 1/sec
```

This is **per-entity throttling** by default:

```rust
// Different user_id values = different signatures = independent throttling
error!(user_id = 123, "Failed to fetch user");  // User 123's quota
error!(user_id = 456, "Failed to fetch user");  // User 456's quota (separate)
```

### Common Per-Entity Patterns

```rust
// Per-endpoint rate limiting
warn!(endpoint = "/api/users", "High latency detected");
warn!(endpoint = "/api/orders", "High latency detected");  // Independent limit

// Per-service monitoring in microservices
error!(service = "auth-service", "Connection timeout");
error!(service = "payment-service", "Connection timeout");  // Separate tracking

// Per-error-code throttling
error!(error_code = "AUTH_FAILED", "Authentication error");
error!(error_code = "TIMEOUT", "Authentication error");  // Different signatures
```

## Best Practice #3: Exclude High-Cardinality Fields

**Problem**: Some fields create too many unique signatures, defeating throttling:

```rust
// ❌ DON'T: request_id creates unique signature for every request
for i in 0..1000 {
    error!(request_id = uuid::Uuid::new_v4().to_string(), "Database timeout");
}
// Result: All 1000 errors logged (each has unique request_id)
```

**Solution**: Exclude high-cardinality fields from signatures:

```rust
// ✅ DO: Exclude request_id so errors are throttled together
let layer = TracingRateLimitLayer::builder()
    .with_excluded_fields(vec!["request_id".to_string(), "trace_id".to_string()])
    .with_policy(Policy::token_bucket(50.0, 1.0).unwrap())
    .build()
    .unwrap();

for i in 0..1000 {
    error!(request_id = uuid::Uuid::new_v4().to_string(), "Database timeout");
}
// Result: First 50 logged, then 1/sec (all share same signature now)
```

### Common High-Cardinality Fields to Exclude

```rust
let layer = TracingRateLimitLayer::builder()
    .with_excluded_fields(vec![
        "request_id".to_string(),
        "trace_id".to_string(),
        "span_id".to_string(),
        "correlation_id".to_string(),
        "timestamp".to_string(),
        "latency_ms".to_string(),
        "duration".to_string(),
    ])
    .build()
    .unwrap();
```

**Rule of Thumb**: If a field has more than ~100 unique values in production, consider excluding it.

## Best Practice #4: Choose the Right Rate Limiting Policy

### Token Bucket (Default) - Recommended for Most Cases

```rust
// Allow bursts of 50 events, then refill at 1 event/sec (60/min)
Policy::token_bucket(50.0, 1.0).unwrap()
```

**Use when**: You want to tolerate occasional bursts but maintain an average rate.

**Example**: Database connection errors - allow initial burst to see the issue, then limit ongoing noise.

### Time-Window - Strict Periodic Limits

```rust
// Allow exactly 10 events per 60-second window
Policy::time_window(10, Duration::from_secs(60)).unwrap()
```

**Use when**: You need predictable limits for dashboards/alerts.

**Example**: "No more than 100 authentication failures per minute" for security monitoring.

### Count-Based - Limit Total Occurrences

```rust
// Allow only 5 events total, then suppress all remaining
Policy::count_based(5).unwrap()
```

**Use when**: You want to see a few examples then stop.

**Example**: Deprecation warnings at startup - see a few, then suppress the rest.

### Exponential Backoff - Progressive Reduction

```rust
// Emit at: 1st, 2nd, 4th, 8th, 16th, 32nd, 64th...
Policy::exponential_backoff()
```

**Use when**: You want to know an issue is ongoing without flooding logs.

**Example**: Retry logic failures - see the pattern without overwhelming output.

## Best Practice #5: Combine Span Context for Richer Signatures

Use span context fields for ambient context that should affect throttling:

```rust
let layer = TracingRateLimitLayer::builder()
    .with_span_context_fields(vec!["user_id".to_string()])
    .with_excluded_fields(vec!["request_id".to_string()])
    .build()
    .unwrap();

let span = info_span!("request", user_id = "alice");
let _enter = span.enter();

// All these errors share: (user_id="alice", error_code="TIMEOUT")
for _ in 0..100 {
    error!(error_code = "TIMEOUT", "Service unavailable");  // Throttled together
}

// Different user = different signature
let span2 = info_span!("request", user_id = "bob");
let _enter2 = span2.enter();

for _ in 0..100 {
    error!(error_code = "TIMEOUT", "Service unavailable");  // Independent quota
}
```

**Use case**: Multi-tenant applications where you want per-tenant rate limiting.

## Best Practice #6: Memory Management for High-Cardinality Scenarios

### Default Settings

- Tracks up to **10,000 unique signatures**
- ~200-400 bytes per signature
- ~2-4 MB typical memory usage

### Adjust Based on Cardinality

```rust
// Low cardinality (few unique log patterns)
let layer = TracingRateLimitLayer::builder()
    .with_max_signatures(1_000)
    .build()
    .unwrap();

// Medium cardinality (per-user throttling, 10k users)
let layer = TracingRateLimitLayer::builder()
    .with_max_signatures(50_000)
    .with_eviction_strategy(EvictionStrategy::lru())
    .build()
    .unwrap();

// High cardinality (per-user per-endpoint, 100k combinations)
let layer = TracingRateLimitLayer::builder()
    .with_max_signatures(100_000)
    .with_eviction_strategy(EvictionStrategy::combined(
        EvictionStrategy::priority(),
        EvictionStrategy::memory_based(50 * 1024 * 1024), // 50 MB limit
    ))
    .build()
    .unwrap();
```

### Memory Estimation

Formula: `max_signatures * 300 bytes ≈ memory usage`

Examples:
- 10,000 signatures ≈ 3 MB
- 50,000 signatures ≈ 15 MB
- 100,000 signatures ≈ 30 MB

## Best Practice #7: Monitor and Observe Throttling Behavior

```rust
let layer = TracingRateLimitLayer::builder()
    .with_active_emission(true)  // Emit suppression summaries
    .with_summary_interval(Duration::from_secs(60))
    .build()
    .unwrap();

let metrics = layer.metrics().clone();

// Periodic metrics reporting
tokio::spawn(async move {
    loop {
        tokio::time::sleep(Duration::from_secs(60)).await;

        let snapshot = metrics.snapshot();
        info!(
            events_allowed = snapshot.events_allowed,
            events_suppressed = snapshot.events_suppressed,
            suppression_rate = format!("{:.1}%", snapshot.suppression_rate() * 100.0),
            active_signatures = snapshot.active_signatures,
            "Rate limiting metrics"
        );
    }
});
```

## Common Anti-Patterns to Avoid

### ❌ Anti-Pattern 1: Treating Different Events as Same

```rust
// WRONG: Deduplicating semantically different errors
let layer = TracingRateLimitLayer::builder()
    .with_excluded_fields(vec!["user_id".to_string()])  // DON'T!
    .build()
    .unwrap();

error!(user_id = 123, "Payment failed");
error!(user_id = 456, "Payment failed");
// Both suppressed together - you lose visibility into which users are affected!
```

**Fix**: Only exclude truly high-cardinality fields like request_id, not semantic identifiers like user_id.

### ❌ Anti-Pattern 2: Too Many Excluded Fields

```rust
// WRONG: Excluding too many fields loses context
let layer = TracingRateLimitLayer::builder()
    .with_excluded_fields(vec![
        "user_id".to_string(),
        "error_code".to_string(),
        "endpoint".to_string(),
        // ... too many!
    ])
    .build()
    .unwrap();
```

**Fix**: Only exclude fields with cardinality > 100. Keep semantic fields.

### ❌ Anti-Pattern 3: Using Dynamic Messages

```rust
// WRONG: Dynamic messages prevent signature matching
error!("Failed after {} retries for user {}", retry_count, user_id);
```

**Fix**: Use static messages with structured fields:

```rust
error!(retry_count = retry_count, user_id = user_id, "Retry limit exceeded");
```

## Testing Your Configuration

```rust
#[cfg(test)]
mod tests {
    use tracing_throttle::*;

    #[test]
    fn test_per_user_throttling() {
        let layer = TracingRateLimitLayer::builder()
            .with_policy(Policy::count_based(2).unwrap())
            .with_excluded_fields(vec!["request_id".to_string()])
            .build()
            .unwrap();

        let metrics = layer.metrics().clone();

        tracing::subscriber::with_default(
            tracing_subscriber::registry().with(layer),
            || {
                // User 123: should allow 2
                for _ in 0..5 {
                    tracing::error!(
                        user_id = 123,
                        request_id = "req-1",
                        "Failed"
                    );
                }

                // User 456: should also allow 2 (independent quota)
                for _ in 0..5 {
                    tracing::error!(
                        user_id = 456,
                        request_id = "req-2",
                        "Failed"
                    );
                }
            },
        );

        // Should allow 4 total: 2 for user 123 + 2 for user 456
        assert_eq!(metrics.events_allowed(), 4);
        assert_eq!(metrics.events_suppressed(), 6);
    }
}
```

## When NOT to Use tracing-throttle

- **Low-volume applications**: < 100 events/sec may not need throttling
- **Critical debugging**: Disable temporarily when investigating specific issues
- **Compliance requirements**: Some scenarios require complete log retention
- **Already deduplicated**: If your log aggregation system handles it

## Migration from v0.3.x to v0.4.0

### Breaking Change: Field Inclusion

**v0.3.x (old)**:
```rust
// Fields excluded by default, opt-in with with_event_fields()
let layer = TracingRateLimitLayer::builder()
    .with_event_fields(vec!["user_id".to_string()])  // REMOVED in v0.4
    .build()
    .unwrap();
```

**v0.4.0 (new)**:
```rust
// All fields included by default, opt-out with with_excluded_fields()
let layer = TracingRateLimitLayer::builder()
    .with_excluded_fields(vec!["request_id".to_string(), "trace_id".to_string()])
    .build()
    .unwrap();
```

**Why this change**: Including all fields by default prevents accidental deduplication of semantically different events. Events with different field values are now correctly treated as distinct.

## Key Takeaways

1. **All field values are included in signatures by default** - different values = different signatures
2. **Keep message strings static** - use structured fields for variable data
3. **Exclude high-cardinality fields** (request_id, trace_id) to prevent signature explosion
4. **Low-cardinality fields define semantics** - keep user_id, error_code, endpoint in signatures
5. **Choose the right policy** - token bucket is a good default for most cases
6. **Monitor metrics** to verify throttling works as expected
7. **Test your configuration** to ensure expected behavior

## Further Reading

- [Official Documentation](https://docs.rs/tracing-throttle)
- [GitHub Repository](https://github.com/nootr/tracing-throttle)
- [tracing Ecosystem](https://github.com/tokio-rs/tracing)
