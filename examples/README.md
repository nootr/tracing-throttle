# tracing-throttle Examples

This directory contains examples demonstrating different features of `tracing-throttle`.

## Available Examples

### Basic Usage

```bash
cargo run --example basic
```

Demonstrates simple rate limiting with default settings.

### Policy Examples

```bash
cargo run --example policies
```

Shows how to use different rate limiting policies:
- Token bucket (burst tolerance with smooth recovery)
- Time window (sliding window rate limiting)
- Count-based (simple event counting)
- Exponential backoff (for extremely noisy logs)

### Redis Storage (Distributed Rate Limiting)

**Requires**: Docker and the `redis-storage` feature

```bash
# Start Redis
cd examples/redis
docker-compose up -d

# Run the example (from project root)
cargo run --example redis --features redis-storage

# Stop Redis when done
cd examples/redis
docker-compose down
```

This example demonstrates:
- Sharing rate limit state across multiple processes via Redis
- Distributed rate limiting for microservices
- Automatic TTL-based cleanup of inactive signatures

**Try running multiple instances** in different terminals to see distributed rate limiting in action!

📖 **See [`examples/redis/README.md`](redis/README.md) for detailed instructions, troubleshooting, and architecture details.**
