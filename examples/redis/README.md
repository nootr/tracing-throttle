# Redis Storage Example

This example demonstrates distributed rate limiting using Redis as the storage backend.

## Quick Start

1. **Start Redis**:
   ```bash
   cd examples/redis
   docker-compose up -d
   ```

2. **Run the example** (from project root):
   ```bash
   cargo run --example redis --features redis-storage
   ```

3. **Stop Redis when done**:
   ```bash
   cd examples/redis
   docker-compose down
   ```

## What This Example Shows

- ✅ Sharing rate limit state across multiple processes via Redis
- ✅ Token bucket rate limiting (50 burst, 5/sec sustained rate)
- ✅ Automatic TTL-based cleanup (5 minute expiration)
- ✅ Different event signatures tracked independently
- ✅ Real distributed rate limiting in action

## Testing Distributed Behavior

Run multiple instances in separate terminals to see how they share rate limits:

```bash
# Terminal 1
cargo run --example redis --features redis-storage

# Terminal 2 (run simultaneously)
cargo run --example redis --features redis-storage
```

**You'll notice**: Both processes consume from the same token bucket! If one process uses 30 tokens, the other only has 20 left (out of the 50 burst capacity).

## Inspecting Redis Data

While the example is running, you can inspect the Redis data:

```bash
# Connect to Redis CLI
docker exec -it tracing-throttle-redis redis-cli

# List all tracing-throttle keys
127.0.0.1:6379> KEYS tracing_throttle:*

# View a specific key (binary data - won't be human readable)
127.0.0.1:6379> GET tracing_throttle:1234567890

# Check TTL (time to live) on a key
127.0.0.1:6379> TTL tracing_throttle:1234567890

# Monitor all Redis commands in real-time
127.0.0.1:6379> MONITOR
```

## Docker Compose Details

The `docker-compose.yml` provides:

- **Image**: `redis:7-alpine` (lightweight, ~10MB)
- **Port**: `6379` (standard Redis port)
- **Persistence**: Enabled with AOF (append-only file)
- **Health checks**: Automatic with `redis-cli ping`
- **Volume**: Named volume `redis-data` for persistence across restarts

## Troubleshooting

### Redis Not Starting

```bash
# Check logs
docker logs tracing-throttle-redis

# Restart
cd examples/redis
docker-compose restart
```

### Port Already in Use

If port 6379 is taken:

```bash
# Find what's using it
lsof -i :6379

# Option 1: Stop the other service
# Option 2: Edit docker-compose.yml to use a different port:
#   ports:
#     - "6380:6379"
# Then update the example to connect to 6380
```

### Connection Refused

Make sure Redis is running and healthy:

```bash
docker ps | grep redis
# Should show "healthy" status
```

## Architecture

```
┌─────────────┐     ┌─────────────┐
│  Process 1  │────▶│             │
│  (Example)  │     │    Redis    │
└─────────────┘     │   (Docker)  │
                    │             │
┌─────────────┐     │             │
│  Process 2  │────▶│             │
│  (Example)  │     │   Port 6379 │
└─────────────┘     └─────────────┘

Both processes read/write the same keys in Redis:
- Key: tracing_throttle:<signature_hash>
- Value: Serialized EventState (bincode)
- TTL: 300 seconds (5 minutes)
```

## Cleanup

Remove all data including volumes:

```bash
cd examples/redis
docker-compose down -v
```

This will delete the persistent Redis data.
