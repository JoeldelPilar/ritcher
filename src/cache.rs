//! Short-TTL origin manifest cache.
//!
//! Deduplicates identical origin fetches across concurrent viewers.
//! The default 2-second TTL is short enough to stay close to live edge
//! while eliminating thundering-herd requests to the origin CDN.
//! The TTL is configurable via [`ManifestCache::with_ttl`] or the
//! `MANIFEST_CACHE_TTL_MS` environment variable.

use dashmap::DashMap;
use metrics::{counter, gauge};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::debug;

/// Default TTL for cached manifests (2 seconds).
const DEFAULT_TTL: Duration = Duration::from_secs(2);

/// Prometheus metric: manifest cache hits.
const CACHE_HIT: &str = "ritcher_manifest_cache_hit";
/// Prometheus metric: manifest cache misses.
const CACHE_MISS: &str = "ritcher_manifest_cache_miss";
/// Prometheus metric: current number of cached entries.
const CACHE_ENTRIES: &str = "ritcher_manifest_cache_entries";

/// A cached origin response with its insertion timestamp.
#[derive(Clone, Debug)]
struct CachedEntry {
    body: String,
    fetched_at: Instant,
}

/// Thread-safe manifest cache with TTL-based invalidation.
///
/// Uses [`DashMap`] for lock-free concurrent reads/writes. Entries older
/// than the configured TTL are lazily evicted on the next `get()` call.
#[derive(Clone, Debug)]
pub struct ManifestCache {
    entries: Arc<DashMap<String, CachedEntry>>,
    ttl: Duration,
}

impl ManifestCache {
    /// Create a new manifest cache with the default 2-second TTL.
    pub fn new() -> Self {
        Self {
            entries: Arc::new(DashMap::new()),
            ttl: DEFAULT_TTL,
        }
    }

    /// Create a new manifest cache with a custom TTL.
    pub fn with_ttl(ttl: Duration) -> Self {
        Self {
            entries: Arc::new(DashMap::new()),
            ttl,
        }
    }

    /// Try to get a cached manifest body for the given origin URL.
    ///
    /// Returns `Some(body)` if a fresh entry exists, `None` otherwise.
    /// Records cache hit/miss metrics on every call.
    pub fn get(&self, url: &str) -> Option<String> {
        if let Some(entry) = self.entries.get(url) {
            if entry.fetched_at.elapsed() < self.ttl {
                debug!("Manifest cache HIT for {}", url);
                counter!(CACHE_HIT).increment(1);
                return Some(entry.body.clone());
            }
            // Stale -- drop the read guard before removing
            drop(entry);
            self.entries.remove(url);
        }
        debug!("Manifest cache MISS for {}", url);
        counter!(CACHE_MISS).increment(1);
        None
    }

    /// Insert a manifest body into the cache.
    ///
    /// Updates the entry gauge after insertion.
    pub fn insert(&self, url: &str, body: String) {
        self.entries.insert(
            url.to_string(),
            CachedEntry {
                body,
                fetched_at: Instant::now(),
            },
        );
        gauge!(CACHE_ENTRIES).set(self.entries.len() as f64);
    }

    /// Return the current number of cached entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Return `true` if the cache contains no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl Default for ManifestCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_hit_within_ttl() {
        let cache = ManifestCache::new();
        cache.insert("https://origin.example.com/live.m3u8", "body".to_string());

        assert_eq!(
            cache.get("https://origin.example.com/live.m3u8"),
            Some("body".to_string())
        );
    }

    #[test]
    fn cache_miss_for_unknown_url() {
        let cache = ManifestCache::new();
        assert_eq!(cache.get("https://unknown.example.com/live.m3u8"), None);
    }

    #[test]
    fn cache_miss_after_ttl() {
        let cache = ManifestCache::with_ttl(Duration::from_millis(1));
        cache.insert("https://origin.example.com/live.m3u8", "body".to_string());

        std::thread::sleep(Duration::from_millis(5));

        assert_eq!(
            cache.get("https://origin.example.com/live.m3u8"),
            None,
            "Entry should be stale after TTL"
        );
    }

    #[test]
    fn cache_overwrite_refreshes_entry() {
        let cache = ManifestCache::new();
        cache.insert("https://origin.example.com/live.m3u8", "old".to_string());
        cache.insert("https://origin.example.com/live.m3u8", "new".to_string());

        assert_eq!(
            cache.get("https://origin.example.com/live.m3u8"),
            Some("new".to_string())
        );
    }

    #[test]
    fn custom_ttl_via_with_ttl() {
        let cache = ManifestCache::with_ttl(Duration::from_secs(60));
        cache.insert("https://origin.example.com/live.m3u8", "body".to_string());

        // Should still be fresh with a 60s TTL
        assert_eq!(
            cache.get("https://origin.example.com/live.m3u8"),
            Some("body".to_string())
        );
    }

    #[test]
    fn len_and_is_empty() {
        let cache = ManifestCache::new();
        assert!(cache.is_empty());
        assert_eq!(cache.len(), 0);

        cache.insert("https://a.example.com/live.m3u8", "a".to_string());
        assert!(!cache.is_empty());
        assert_eq!(cache.len(), 1);

        cache.insert("https://b.example.com/live.m3u8", "b".to_string());
        assert_eq!(cache.len(), 2);
    }
}
