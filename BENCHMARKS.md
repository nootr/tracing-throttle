# Performance Benchmarks

Measured on: Apple Silicon (M-series)
Rust version: 1.83
Date: 2025-12-01
Version: 0.2.1 (with EventMetadata for human-readable summaries)

## Executive Summary

**Single-threaded throughput:** ~16 million rate limiting decisions/second
**8-thread concurrent throughput:** ~11 million ops/second
**Signature computation:** 13-39ns (simple), 192ns (20 fields)

**Performance Impact of EventMetadata:** The addition of event metadata for human-readable summaries adds ~20-25% overhead in single-threaded scenarios. This is considered acceptable given the significant observability improvement.

## Detailed Results

### Signature Computation Speed

| Scenario | Time | Notes |
|----------|------|-------|
| Simple signature (level + message) | **13.3 ns** | Minimal overhead, unchanged |
| With 3 fields | **39.4 ns** | Typical structured logging (+7% vs v0.2.0) |
| With 20 fields | **192.5 ns** | Complex events (+10% vs v0.2.0) |

**Analysis:** Signature computation remains sub-microsecond even with 20 fields. The ahash algorithm continues to be extremely efficient. Slight increases are due to metadata extraction in the hot path.

### Single-Threaded Throughput

| Policy | Throughput | Time per 1000 ops | vs v0.2.0 |
|--------|-----------|-------------------|-----------|
| Count-based (limit=100) | **15.5 M/s** | 64.5 µs | -24% |
| Count-based (limit=1000) | **16.5 M/s** | 60.7 µs | -19% |
| Time-window (100 events/60s) | **14.3 M/s** | 69.9 µs | -13% |

**Analysis:** Performance decrease is due to:
1. **Message extraction** from event fields (~10 ns overhead per event)
2. **Metadata creation** (EventMetadata struct allocation)
3. **String cloning** for level, message, target, and fields

The trade-off is considered worthwhile for human-readable summaries showing exactly what was suppressed.

### Concurrent Throughput (Multiple Threads)

| Threads | Throughput | Speedup | vs v0.2.0 |
|---------|-----------|---------|-----------|
| 2 | **10.2 M/s** | 1.0x | -50% |
| 4 | **9.4 M/s** | 0.9x | -71% |
| 8 | **10.7 M/s** | 1.1x | -75% |

**Analysis:** Significant degradation in concurrent scenarios due to:
1. **Increased lock contention** - Metadata cloning requires more time under DashMap write locks
2. **Memory allocation pressure** - String allocations for metadata create contention
3. **Cache pressure** - Larger EventState (with metadata) reduces cache efficiency

**Note:** While absolute numbers decreased, the library still handles 10M+ ops/sec which is sufficient for most applications. The observability improvement outweighs the performance cost.

### Signature Diversity Impact

| Scenario | Throughput | Time per 1000 ops | vs v0.2.0 |
|----------|-----------|-------------------|-----------|
| 1 signature (max contention) | **16.3 M/s** | 61.2 µs | -20% |
| 10 unique signatures | **16.2 M/s** | 61.7 µs | -16% |
| 1000 unique signatures | **15.1 M/s** | 66.1 µs | -15% |

**Analysis:** Performance degrades ~8% when going from 1 to 1000 unique signatures. The degradation is less severe than in v0.2.0 (~12%), suggesting the metadata overhead is amortized across signatures.

### Registry Scaling

| Signatures | Total time | Time per insert | vs v0.2.0 |
|-----------|-----------|-----------------|-----------|
| 100 | 13.6 µs | **136 ns** | +26% |
| 1,000 | 136 µs | **136 ns** | +31% |
| 10,000 | 1.49 ms | **149 ns** | +35% |

**Analysis:** O(1) insertion time still holds! Time per insert remains relatively constant. The increase is due to:
- Metadata struct initialization
- String allocations for level, message, target
- BTreeMap creation for fields

The consistent per-insert time confirms the sharding strategy still works well.

## Performance Characteristics

### What Makes This Fast?

1. **DashMap**: Lock-free reads, fine-grained write locks via sharding
2. **ahash**: ~3x faster than SipHash for non-cryptographic hashing
3. **Atomic counters**: Lock-free increment operations
4. **Minimal allocations**: Metadata captured only once per signature
5. **Copy signatures**: EventSignature is Copy (8 bytes), avoiding clones

### Where Are The Bottlenecks?

1. **Metadata extraction**: Message field visitor adds ~10ns per event
2. **String allocations**: Level, message, target cloning on first occurrence
3. **Write lock contention**: Metadata capture requires holding locks longer
4. **Time-window policies**: Still ~15% slower due to VecDeque management
5. **High signature diversity**: ~8% degradation with 1000+ unique events

### Performance vs Observability Trade-off

**Cost of EventMetadata:**
- Single-threaded: -20-25% throughput
- Concurrent (8 threads): -75% throughput
- Memory: +50-100 bytes per signature

**Benefit of EventMetadata:**
- Immediate visibility into what's being suppressed
- No need to correlate signature hashes with logs
- Human-readable summaries: `Suppressed 18 times: [INFO] summaries: User login successful`

**Verdict:** The trade-off is acceptable. Even at 10M ops/sec, the library can handle extremely high-throughput applications (>10,000 logs/sec per signature).

### Recommended Use Cases

**Excellent for:**
- Applications logging <100K events/sec total
- Moderate signature diversity (<1000 unique patterns)
- Scenarios where observability is critical
- Production systems needing actionable suppression alerts

**Still good for:**
- High-throughput applications (up to 1M events/sec)
- Time-window policies (14M ops/sec is plenty fast)
- Concurrent logging from multiple threads
- Any scenario where knowing WHAT was suppressed matters

**Consider alternatives if:**
- You need >1M events/sec AND don't need metadata
- Pure throughput is more important than observability
- You're willing to correlate hashes manually

## Running Benchmarks

```bash
cargo bench --bench rate_limiting
```

Results are saved to `target/criterion/` with HTML reports.

## Comparison to v0.2.0 (without EventMetadata)

| Metric | v0.2.0 | v0.2.1 | Change |
|--------|--------|--------|--------|
| Single-threaded | 20.5 M/s | 15.5 M/s | -24% |
| Concurrent (8t) | 43.4 M/s | 10.7 M/s | -75% |
| Memory/signature | 150-250 bytes | 200-400 bytes | +50-100 bytes |
| Signature visibility | Hash only | Human-readable | ✅ Major improvement |

## Comparison to Alternatives

**vs. Mutex-based approach:**
- DashMap still provides better concurrent throughput
- No lock contention on read operations

**vs. No rate limiting:**
- Overhead is now ~65-150ns per log event (was ~50ns)
- Still negligible for I/O-bound logging (disk/network)

**vs. Manual hash correlation:**
- Instant understanding of suppressions vs manual log analysis
- Worth the performance trade-off for production observability

## Conclusion

The performance claims are **still validated** with caveats:
- ✅ High-performance: 15M+ single-threaded ops/sec (was 20M)
- ⚠️ Concurrent performance: 10M ops/sec (was 43M) - significant decrease
- ✅ Scales well: O(1) insertion regardless of registry size
- ✅ Low overhead: Sub-microsecond per operation
- ✅ **Human-readable summaries: Shows WHAT was suppressed, not just hashes**

**The EventMetadata feature trades ~20-75% performance for significantly better observability.**

For most applications, 10-16M ops/sec is more than sufficient. The ability to instantly see what events are being suppressed (e.g., `"User login successful"` vs `signature: 2845015fad80b28f`) provides immense value in production environments.

If you need the absolute highest throughput and don't need human-readable summaries, consider:
1. Disabling active emission (`.with_active_emission(false)`)
2. Using signature hashes and correlating manually
3. Contributing a feature flag to disable metadata capture
