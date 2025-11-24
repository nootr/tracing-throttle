use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use std::collections::BTreeMap;
use std::sync::Arc;
use tracing_throttle::{
    EventSignature, Policy, RateLimiter, ShardedStorage, SuppressionRegistry, SystemClock,
};

/// Benchmark signature computation speed
fn bench_signature_computation(c: &mut Criterion) {
    let mut group = c.benchmark_group("signature_computation");

    let fields = BTreeMap::from([
        ("user".to_string(), "alice".to_string()),
        ("action".to_string(), "login".to_string()),
        ("ip".to_string(), "192.168.1.1".to_string()),
    ]);

    group.bench_function("simple_signature", |b| {
        b.iter(|| EventSignature::simple(black_box("INFO"), black_box("User logged in")))
    });

    group.bench_function("signature_with_fields", |b| {
        b.iter(|| {
            EventSignature::new(
                black_box("INFO"),
                black_box("User logged in"),
                black_box(&fields),
                Some(black_box("auth::handler")),
            )
        })
    });

    group.bench_function("signature_with_many_fields", |b| {
        let many_fields: BTreeMap<String, String> = (0..20)
            .map(|i| (format!("field{}", i), format!("value{}", i)))
            .collect();

        b.iter(|| {
            EventSignature::new(
                black_box("INFO"),
                black_box("Complex event"),
                black_box(&many_fields),
                Some(black_box("module::path")),
            )
        })
    });

    group.finish();
}

/// Benchmark single-threaded rate limiting throughput
fn bench_single_threaded_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("single_threaded");

    for policy_type in ["count_100", "count_1000", "time_window"].iter() {
        let policy = match *policy_type {
            "count_100" => Policy::count_based(100),
            "count_1000" => Policy::count_based(1000),
            "time_window" => Policy::time_window(100, std::time::Duration::from_secs(60)),
            _ => unreachable!(),
        };

        group.throughput(Throughput::Elements(1000));

        group.bench_with_input(
            BenchmarkId::new("rate_limit_decisions", policy_type),
            &policy,
            |b, policy| {
                let storage = Arc::new(ShardedStorage::new());
                let registry = SuppressionRegistry::new(storage, policy.clone());
                let clock = Arc::new(SystemClock::new());
                let limiter = RateLimiter::new(registry, clock);
                let sig = EventSignature::simple("INFO", "Test message");

                b.iter(|| {
                    for _ in 0..1000 {
                        black_box(limiter.check_event(black_box(sig)));
                    }
                })
            },
        );
    }

    group.finish();
}

/// Benchmark multi-threaded concurrent throughput
fn bench_concurrent_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("concurrent");

    for num_threads in [2, 4, 8].iter() {
        group.throughput(Throughput::Elements((*num_threads as u64) * 1000));

        group.bench_with_input(
            BenchmarkId::new("threads", num_threads),
            num_threads,
            |b, &num_threads| {
                b.iter(|| {
                    let storage = Arc::new(ShardedStorage::new());
                    let policy = Policy::count_based(100);
                    let registry = SuppressionRegistry::new(storage, policy);
                    let clock = Arc::new(SystemClock::new());
                    let limiter = Arc::new(RateLimiter::new(registry, clock));

                    let mut handles = vec![];
                    for i in 0..num_threads {
                        let limiter = Arc::clone(&limiter);
                        let handle = std::thread::spawn(move || {
                            // Each thread uses a different signature to avoid contention
                            let sig = EventSignature::simple("INFO", &format!("Message {}", i));
                            for _ in 0..1000 {
                                black_box(limiter.check_event(black_box(sig)));
                            }
                        });
                        handles.push(handle);
                    }

                    for handle in handles {
                        handle.join().unwrap();
                    }
                })
            },
        );
    }

    group.finish();
}

/// Benchmark different signature scenarios
fn bench_signature_diversity(c: &mut Criterion) {
    let mut group = c.benchmark_group("signature_diversity");
    group.throughput(Throughput::Elements(1000));

    // Single signature (worst case - maximum contention)
    group.bench_function("single_signature", |b| {
        let storage = Arc::new(ShardedStorage::new());
        let policy = Policy::count_based(100);
        let registry = SuppressionRegistry::new(storage, policy);
        let clock = Arc::new(SystemClock::new());
        let limiter = RateLimiter::new(registry, clock);
        let sig = EventSignature::simple("INFO", "Same message");

        b.iter(|| {
            for _ in 0..1000 {
                black_box(limiter.check_event(black_box(sig)));
            }
        })
    });

    // 10 unique signatures (moderate diversity)
    group.bench_function("10_signatures", |b| {
        let storage = Arc::new(ShardedStorage::new());
        let policy = Policy::count_based(100);
        let registry = SuppressionRegistry::new(storage, policy);
        let clock = Arc::new(SystemClock::new());
        let limiter = RateLimiter::new(registry, clock);
        let sigs: Vec<_> = (0..10)
            .map(|i| EventSignature::simple("INFO", &format!("Message {}", i)))
            .collect();

        b.iter(|| {
            for i in 0..1000 {
                let sig = sigs[i % 10];
                black_box(limiter.check_event(black_box(sig)));
            }
        })
    });

    // 1000 unique signatures (maximum diversity - best case)
    group.bench_function("1000_signatures", |b| {
        let storage = Arc::new(ShardedStorage::new());
        let policy = Policy::count_based(100);
        let registry = SuppressionRegistry::new(storage, policy);
        let clock = Arc::new(SystemClock::new());
        let limiter = RateLimiter::new(registry, clock);
        let sigs: Vec<_> = (0..1000)
            .map(|i| EventSignature::simple("INFO", &format!("Message {}", i)))
            .collect();

        b.iter(|| {
            for i in 0..1000 {
                let sig = sigs[i];
                black_box(limiter.check_event(black_box(sig)));
            }
        })
    });

    group.finish();
}

/// Benchmark memory overhead
fn bench_registry_size(c: &mut Criterion) {
    let mut group = c.benchmark_group("registry_scaling");

    for num_signatures in [100, 1000, 10_000].iter() {
        group.bench_with_input(
            BenchmarkId::new("insert", num_signatures),
            num_signatures,
            |b, &num_sigs| {
                b.iter(|| {
                    let storage = Arc::new(ShardedStorage::new());
                    let policy = Policy::count_based(100);
                    let registry = SuppressionRegistry::new(storage, policy);
                    let clock = Arc::new(SystemClock::new());
                    let limiter = RateLimiter::new(registry, clock);

                    for i in 0..num_sigs {
                        let sig = EventSignature::simple("INFO", &format!("Message {}", i));
                        limiter.check_event(sig);
                    }
                })
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_signature_computation,
    bench_single_threaded_throughput,
    bench_concurrent_throughput,
    bench_signature_diversity,
    bench_registry_size,
);
criterion_main!(benches);
