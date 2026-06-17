# Module: `cache`

Source: `src/cache.rs`

Short-TTL in-memory cache for origin manifest bodies. Deduplicates concurrent origin fetches when many players request the same manifest within the same polling window.

## `ManifestCache`

```rust
ManifestCache {
    entries: Arc<DashMap<String, CachedEntry>>,
    ttl: Duration,  // default: 2000ms
}
```

Uses `dashmap::DashMap` for lock-free concurrent reads and writes.

## Methods

| Method | Description |
|--------|-------------|
| `ManifestCache::new()` | 2s TTL |
| `ManifestCache::with_ttl(d)` | Custom TTL |
| `get(url) -> Option<String>` | Returns cached body if fresh; evicts stale entry |
| `insert(url, body)` | Stores body with current timestamp |
| `len() / is_empty()` | Entry count |

## Eviction

Lazy: entries are evicted on the next `get()` call if `fetched_at.elapsed() >= ttl`. There is no proactive sweep — the cache is bounded in practice by the number of distinct origin URLs a Ritcher instance serves (typically one).

## Metrics

| Metric | Recorded on |
|--------|------------|
| `ritcher_manifest_cache_hit` (counter) | `get()` hit |
| `ritcher_manifest_cache_miss` (counter) | `get()` miss |
| `ritcher_manifest_cache_entries` (gauge) | after `insert()` |

## TTL rationale

The default 2s TTL is short enough to stay close to the live edge (a typical HLS segment is 2–6s) while eliminating thundering-herd requests from concurrent viewers polling the same manifest at the same moment. Configurable via `MANIFEST_CACHE_TTL_MS`.
