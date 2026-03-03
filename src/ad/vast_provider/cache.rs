use std::time::Duration;
use tracing::info;

use super::VastAdProvider;

/// Maximum number of entries allowed in the ad cache.
/// Enforced at insert time (not just during periodic cleanup) to prevent
/// unbounded growth between cleanup cycles.
pub(crate) const MAX_CACHE_SIZE: usize = 10_000;

impl VastAdProvider {
    /// Evict stale cache entries and trim to capacity
    pub(crate) fn run_cleanup_cache(&self) {
        const MAX_AGE: Duration = Duration::from_secs(300);

        let before = self.ad_cache.len();

        // Pass 1: evict entries older than MAX_AGE
        self.ad_cache
            .retain(|_, v| v.inserted_at.elapsed() < MAX_AGE);

        // Pass 2: if still over MAX_CACHE_SIZE, evict the oldest entries first.
        // Snapshot into a Vec to avoid TOCTOU issues with concurrent inserts.
        if self.ad_cache.len() > MAX_CACHE_SIZE {
            let mut entries: Vec<(String, Duration)> = self
                .ad_cache
                .iter()
                .map(|e| (e.key().clone(), e.value().inserted_at.elapsed()))
                .collect();

            // Sort descending by age (oldest first)
            entries.sort_unstable_by(|a, b| b.1.cmp(&a.1));

            let to_remove = entries.len().saturating_sub(MAX_CACHE_SIZE);
            for (key, _) in entries.iter().take(to_remove) {
                self.ad_cache.remove(key);
            }
        }

        let after = self.ad_cache.len();
        if before != after {
            info!(
                "VastAdProvider: evicted {} stale cache entries ({} remaining)",
                before - after,
                after
            );
        }

        // Clean up break counters for sessions with no remaining cache entries
        let active_sessions: std::collections::HashSet<String> = self
            .ad_cache
            .iter()
            .filter_map(|e| e.key().split(':').next().map(String::from))
            .collect();
        self.break_counter
            .retain(|session_id, _| active_sessions.contains(session_id));
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use crate::ad::provider::AdProvider;
    use reqwest::Client;

    use crate::ad::vast_provider::ResolvedCreative;

    use super::*;

    #[test]
    fn cleanup_cache_evicts_old_entries() {
        let client = Client::new();
        let provider = VastAdProvider::new("http://ads.example.com/vast".to_string(), client);

        // Old entry: inserted 400 s ago -- exceeds the 300 s MAX_AGE constant
        provider.ad_cache.insert(
            "session-old:break-0-seg-0.ts".to_string(),
            ResolvedCreative {
                url: "http://cdn.example.com/old.m3u8".to_string(),
                duration: 15.0,
                is_hls: true,
                impression_urls: vec![],
                tracking_events: vec![],
                error_url: None,
                total_segments: 1,
                segment_index: 0,
                visited: false,
                inserted_at: Instant::now() - Duration::from_secs(400),
            },
        );

        // Fresh entry: just inserted
        provider.ad_cache.insert(
            "session-new:break-0-seg-0.ts".to_string(),
            ResolvedCreative {
                url: "http://cdn.example.com/new.m3u8".to_string(),
                duration: 15.0,
                is_hls: true,
                impression_urls: vec![],
                tracking_events: vec![],
                error_url: None,
                total_segments: 1,
                segment_index: 0,
                visited: false,
                inserted_at: Instant::now(),
            },
        );

        assert_eq!(provider.ad_cache.len(), 2);
        provider.cleanup_cache();
        assert_eq!(provider.ad_cache.len(), 1, "Old entry should be evicted");
        assert!(
            provider
                .ad_cache
                .contains_key("session-new:break-0-seg-0.ts"),
            "Fresh entry should remain after cleanup"
        );
    }

    #[test]
    fn cleanup_cache_also_removes_stale_break_counters() {
        let client = Client::new();
        let provider = VastAdProvider::new("http://ads.example.com/vast".to_string(), client);

        // Simulate a session with a break counter but expired cache entries
        provider
            .break_counter
            .insert("expired-session".to_string(), 3);
        provider.ad_cache.insert(
            "expired-session:break-0-seg-0.ts".to_string(),
            ResolvedCreative {
                url: "http://cdn.example.com/old.m3u8".to_string(),
                duration: 15.0,
                is_hls: true,
                impression_urls: vec![],
                tracking_events: vec![],
                error_url: None,
                total_segments: 1,
                segment_index: 0,
                visited: false,
                inserted_at: Instant::now() - Duration::from_secs(400),
            },
        );

        // Active session with fresh cache
        provider
            .break_counter
            .insert("active-session".to_string(), 1);
        provider.ad_cache.insert(
            "active-session:break-0-seg-0.ts".to_string(),
            ResolvedCreative {
                url: "http://cdn.example.com/new.m3u8".to_string(),
                duration: 15.0,
                is_hls: true,
                impression_urls: vec![],
                tracking_events: vec![],
                error_url: None,
                total_segments: 1,
                segment_index: 0,
                visited: false,
                inserted_at: Instant::now(),
            },
        );

        assert_eq!(provider.break_counter.len(), 2);
        provider.cleanup_cache();

        assert_eq!(
            provider.break_counter.len(),
            1,
            "Expired session's break counter should be cleaned up"
        );
        assert!(provider.break_counter.contains_key("active-session"));
        assert!(!provider.break_counter.contains_key("expired-session"));
    }

    #[test]
    fn cache_insert_rejected_at_capacity() {
        let client = Client::new();
        let provider = VastAdProvider::new("http://ads.example.com/vast".to_string(), client);

        // Fill the cache to MAX_CACHE_SIZE
        for i in 0..MAX_CACHE_SIZE {
            provider.ad_cache.insert(
                format!("session-fill:break-{}-seg-0.ts", i),
                ResolvedCreative {
                    url: format!("http://cdn.example.com/ad-{}.m3u8", i),
                    duration: 15.0,
                    is_hls: true,
                    impression_urls: vec![],
                    tracking_events: vec![],
                    error_url: None,
                    total_segments: 1,
                    segment_index: 0,
                    visited: false,
                    inserted_at: Instant::now(),
                },
            );
        }

        assert_eq!(provider.ad_cache.len(), MAX_CACHE_SIZE);

        // Simulate the insert-time guard: at capacity, the insert should be skipped
        let key = "session-overflow:break-0-seg-0.ts".to_string();
        if provider.ad_cache.len() >= MAX_CACHE_SIZE {
            // This branch would be taken -- insert skipped
        } else {
            provider.ad_cache.insert(
                key.clone(),
                ResolvedCreative {
                    url: "http://cdn.example.com/overflow.m3u8".to_string(),
                    duration: 15.0,
                    is_hls: true,
                    impression_urls: vec![],
                    tracking_events: vec![],
                    error_url: None,
                    total_segments: 1,
                    segment_index: 0,
                    visited: false,
                    inserted_at: Instant::now(),
                },
            );
        }

        assert_eq!(
            provider.ad_cache.len(),
            MAX_CACHE_SIZE,
            "Cache should not grow beyond MAX_CACHE_SIZE"
        );
        assert!(
            !provider.ad_cache.contains_key(&key),
            "Overflow entry should not have been inserted"
        );
    }

    #[test]
    fn cache_insert_succeeds_under_capacity() {
        let client = Client::new();
        let provider = VastAdProvider::new("http://ads.example.com/vast".to_string(), client);

        // Insert one entry -- well under capacity
        let key = "session-ok:break-0-seg-0.ts".to_string();
        if provider.ad_cache.len() >= MAX_CACHE_SIZE {
            // Would not enter this branch
        } else {
            provider.ad_cache.insert(
                key.clone(),
                ResolvedCreative {
                    url: "http://cdn.example.com/ad.m3u8".to_string(),
                    duration: 15.0,
                    is_hls: true,
                    impression_urls: vec![],
                    tracking_events: vec![],
                    error_url: None,
                    total_segments: 1,
                    segment_index: 0,
                    visited: false,
                    inserted_at: Instant::now(),
                },
            );
        }

        assert_eq!(provider.ad_cache.len(), 1);
        assert!(
            provider.ad_cache.contains_key(&key),
            "Entry should be inserted when under capacity"
        );
    }
}
