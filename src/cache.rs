//! Short-TTL origin manifest cache.
//!
//! Deduplicates identical origin fetches across concurrent viewers.
//! A 2-second TTL is short enough to stay close to live edge while
//! eliminating thundering-herd requests to the origin CDN.

use dashmap::DashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::debug;

/// Default TTL for cached manifests.
const DEFAULT_TTL: Duration = Duration::from_secs(2);

/// A cached origin response.
#[derive(Clone, Debug)]
struct CachedEntry {
    body: String,
    fetched_at: Instant,
}

/// Thread-safe manifest cache with TTL-based invalidation.
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

    /// Try to get a cached manifest body for the given origin URL.
    ///
    /// Returns `Some(body)` if a fresh entry exists, `None` otherwise.
    pub fn get(&self, url: &str) -> Option<String> {
        if let Some(entry) = self.entries.get(url) {
            if entry.fetched_at.elapsed() < self.ttl {
                debug!("Manifest cache HIT for {}", url);
                return Some(entry.body.clone());
            }
            // Stale — drop the read guard before removing
            drop(entry);
            self.entries.remove(url);
        }
        debug!("Manifest cache MISS for {}", url);
        None
    }

    /// Insert a manifest body into the cache.
    pub fn insert(&self, url: &str, body: String) {
        self.entries.insert(
            url.to_string(),
            CachedEntry {
                body,
                fetched_at: Instant::now(),
            },
        );
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
        let cache = ManifestCache {
            entries: Arc::new(DashMap::new()),
            ttl: Duration::from_millis(1),
        };
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
}
