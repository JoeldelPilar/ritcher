# Module: `session`

Source: `src/session/`

Per-viewer session management. A **Session** is a logical record keyed by session ID, holding the origin URL and timestamps. It outlives individual HTTP requests.

## `Session`

```
Session {
    session_id: String
    origin_url: String
    created_at: DateTime<Utc>
    last_accessed: DateTime<Utc>
}
```

Sessions are created (or refreshed) on each manifest request. The TTL is measured from `last_accessed`, not `created_at`.

## `SessionManager`

The public interface for session operations:

```rust
SessionManager::new_memory(ttl: Duration) -> Self
SessionManager::new_valkey(url: &str, ttl: Duration) -> Result<Self>

async fn get(&self, id: &str) -> Option<Session>
async fn upsert(&self, id: &str, origin_url: &str) -> Session
async fn cleanup_expired(&self)
```

`cleanup_expired` is called every 60s by the background task. On the memory backend it walks the `DashMap` and removes entries whose `last_accessed` exceeds the TTL. The Valkey backend delegates TTL eviction to the Redis `EXPIRE` key command.

## `SessionStore` trait

```rust
#[async_trait]
trait SessionStore: Send + Sync {
    async fn get(&self, id: &str) -> Result<Option<Session>, SessionError>;
    async fn set(&self, session: &Session) -> Result<(), SessionError>;
    async fn remove(&self, id: &str) -> Result<(), SessionError>;
    async fn cleanup_expired(&self) -> Result<usize, SessionError>;
}
```

Source: `src/session/store.rs`

## Memory backend

Source: `src/session/memory.rs`

`DashMap<String, Session>` — lock-free concurrent access. Chosen because manifest requests for the same session arrive concurrently from the same player (multiple quality levels).

## Valkey backend

Source: `src/session/valkey.rs`

Optional, compiled only with `--features valkey`. Uses `redis 0.29` with `tokio-comp` for async I/O and `connection-manager` for automatic reconnect.

Sessions are serialized to JSON and stored with `SET key value EX ttl_secs`. Cleanup is a no-op on the Valkey backend — the Redis `EXPIRE` handles eviction natively.

**When to use**: horizontal scaling behind a load balancer, where a player's requests can route to different Ritcher instances. Without a shared store, a player hitting a different instance sees no session and triggers a fresh origin fetch + VAST request, doubling latency on the first manifest.

## Configuration

| Env var | Effect |
|---------|--------|
| `SESSION_STORE=memory` | Default. In-process `DashMap`. |
| `SESSION_STORE=valkey` | Requires `--features valkey` at build time and `VALKEY_URL`. |
| `SESSION_TTL_SECS` | TTL in seconds (default: 300). |
| `VALKEY_URL` | Connection URL, e.g. `redis://valkey:6379`. |
