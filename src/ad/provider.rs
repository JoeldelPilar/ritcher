use crate::ad::vast::TrackingEvent;
use tracing::info;

/// Represents a single ad segment
#[derive(Debug, Clone, PartialEq)]
pub struct AdSegment {
    /// URI of the ad segment
    pub uri: String,
    /// Duration of the segment in seconds
    pub duration: f32,
    /// Tracking metadata (only present for VAST-sourced ads)
    pub tracking: Option<AdTrackingInfo>,
}

/// Tracking metadata for a single ad creative
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AdTrackingInfo {
    /// Impression URLs to fire when this ad first starts
    pub impression_urls: Vec<String>,
    /// Quartile/progress tracking events
    pub tracking_events: Vec<TrackingEvent>,
    /// Error URL to fire on failures
    pub error_url: Option<String>,
    /// Total number of segments in this ad (for quartile calculation)
    pub total_segments: usize,
    /// Index of this segment within the ad
    pub segment_index: usize,
}

/// Resolved segment with optional tracking context
#[derive(Debug, Clone)]
pub struct ResolvedSegment {
    /// URL to the ad segment source
    pub url: String,
    /// Tracking context (if available and not yet fired)
    pub tracking: Option<AdTrackingInfo>,
}

/// Ad creative for Server-Guided Ad Insertion (SGAI).
///
/// Unlike `AdSegment` (single TS segment), `AdCreative` represents a complete
/// ad unit (HLS master/media playlist or MP4 URL) as served in the
/// HLS Interstitials asset-list JSON (`ASSETS` array).
#[derive(Debug, Clone)]
pub struct AdCreative {
    /// URI of the ad creative (HLS playlist URL or MP4 URL)
    pub uri: String,
    /// Duration of the creative in seconds
    pub duration: f64,
}

/// Trait for ad content providers
///
/// Implementations provide ad segments to fill ad breaks of a given duration.
/// This abstraction allows for different ad decision strategies (static, VAST, VMAP, etc.)
pub trait AdProvider: Send + Sync {
    /// Get ad segments to fill an ad break of the given duration
    ///
    /// # Arguments
    /// * `duration` - Duration of the ad break in seconds
    /// * `session_id` - Session ID for tracking and personalization
    ///
    /// # Returns
    /// A vector of AdSegment structs. The total duration may be less than, equal to,
    /// or slightly greater than the requested duration.
    fn get_ad_segments(&self, duration: f32, session_id: &str) -> Vec<AdSegment>;

    /// Resolve an ad segment identifier to its actual source URL
    ///
    /// The ad handler receives ad segment identifiers (e.g. "break-0-seg-3.ts")
    /// and uses this method to get the actual URL to fetch the segment from.
    /// This keeps the handler decoupled from ad source implementation details.
    ///
    /// # Arguments
    /// * `ad_name` - Ad segment identifier from the playlist
    ///
    /// # Returns
    /// Full URL to the ad segment, or None if the ad_name is invalid
    fn resolve_segment_url(&self, ad_name: &str) -> Option<String>;

    /// Resolve segment URL and return tracking context (if available)
    ///
    /// Default implementation calls resolve_segment_url and returns no tracking info.
    /// VAST provider overrides this to return tracking metadata on first access.
    ///
    /// # Arguments
    /// * `ad_name` - Ad segment identifier
    /// * `session_id` - Session ID
    ///
    /// # Returns
    /// ResolvedSegment with URL and optional tracking context
    fn resolve_segment_with_tracking(
        &self,
        ad_name: &str,
        _session_id: &str,
    ) -> Option<ResolvedSegment> {
        self.resolve_segment_url(ad_name)
            .map(|url| ResolvedSegment {
                url,
                tracking: None,
            })
    }

    /// Get ad creatives for SGAI asset-list responses.
    ///
    /// Returns a list of `AdCreative` items that map directly to entries in the
    /// HLS Interstitials asset-list JSON (`ASSETS` array). Each creative is a
    /// complete ad unit (HLS playlist or MP4 URL), not an individual segment.
    ///
    /// Default implementation adapts the SSAI segment list — one creative per
    /// segment. VAST provider overrides this to return proper creative-level URLs.
    fn get_ad_creatives(&self, duration: f32, session_id: &str) -> Vec<AdCreative> {
        self.get_ad_segments(duration, session_id)
            .into_iter()
            .map(|seg| AdCreative {
                uri: seg.uri,
                duration: seg.duration as f64,
            })
            .collect()
    }
}

/// Static ad provider that returns a fixed set of ad segments
///
/// This is the MVP implementation that uses a configured ad source URL
/// and segment duration to generate ad segments.
#[derive(Clone, Debug)]
pub struct StaticAdProvider {
    /// Base URL for ad segments
    ad_source_url: String,
    /// Duration of each ad segment
    segment_duration: f32,
    /// Number of available segments in the ad source (for cycling)
    segment_count: usize,
}

impl StaticAdProvider {
    /// Create a new StaticAdProvider
    ///
    /// # Arguments
    /// * `ad_source_url` - Base URL where ad segments are hosted
    /// * `segment_duration` - Duration of each ad segment in seconds
    pub fn new(ad_source_url: String, segment_duration: f32) -> Self {
        Self::with_segment_count(ad_source_url, segment_duration, 10)
    }

    /// Create a new StaticAdProvider with custom segment count
    ///
    /// # Arguments
    /// * `ad_source_url` - Base URL where ad segments are hosted
    /// * `segment_duration` - Duration of each ad segment in seconds
    /// * `segment_count` - Number of segments available in the ad source
    pub fn with_segment_count(
        ad_source_url: String,
        segment_duration: f32,
        segment_count: usize,
    ) -> Self {
        Self {
            ad_source_url,
            segment_duration,
            segment_count,
        }
    }

    /// Parse segment index from ad name like "break-0-seg-3.ts" → Some(3)
    fn parse_segment_index(&self, ad_name: &str) -> Option<usize> {
        let name = ad_name.strip_suffix(".ts").unwrap_or(ad_name);
        let parts: Vec<&str> = name.split('-').collect();

        // Expected format: ["break", "0", "seg", "3"]
        if parts.len() >= 4 && parts[0] == "break" && parts[2] == "seg" {
            parts[3].parse().ok()
        } else {
            None
        }
    }
}

impl AdProvider for StaticAdProvider {
    fn get_ad_segments(&self, duration: f32, session_id: &str) -> Vec<AdSegment> {
        info!(
            "StaticAdProvider: Generating ad segments for session {} with duration {}s",
            session_id, duration
        );

        // Calculate how many segments we need to fill the duration
        let num_segments = (duration / self.segment_duration).ceil() as usize;
        let num_segments = num_segments.max(1); // At least one segment

        // Generate ad segments
        let segments: Vec<AdSegment> = (0..num_segments)
            .map(|i| AdSegment {
                // For MVP, all segments point to the same ad source
                // In production, this would be different ad creatives
                uri: format!("{}/ad-segment-{}.ts", self.ad_source_url, i),
                duration: self.segment_duration,
                tracking: None,
            })
            .collect();

        info!(
            "StaticAdProvider: Generated {} ad segments (total duration: {}s)",
            segments.len(),
            segments.len() as f32 * self.segment_duration
        );

        segments
    }

    fn resolve_segment_url(&self, ad_name: &str) -> Option<String> {
        let seg_index = self.parse_segment_index(ad_name)?;

        // Map to ad source segment name, cycling through available segments
        // Ad source uses naming like "out_000.ts", "out_001.ts", etc.
        let source_index = seg_index % self.segment_count;
        let source_segment = format!("out_{:03}.ts", source_index);

        Some(format!("{}/{}", self.ad_source_url, source_segment))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_static_ad_provider_exact_duration() {
        let provider = StaticAdProvider::new("https://ads.example.com".to_string(), 10.0);
        let segments = provider.get_ad_segments(30.0, "test-session");

        assert_eq!(segments.len(), 3);
        assert_eq!(segments[0].duration, 10.0);
        assert_eq!(segments[0].uri, "https://ads.example.com/ad-segment-0.ts");
        assert_eq!(segments[0].tracking, None);
        assert_eq!(segments[1].uri, "https://ads.example.com/ad-segment-1.ts");
        assert_eq!(segments[2].uri, "https://ads.example.com/ad-segment-2.ts");
    }

    #[test]
    fn test_static_ad_provider_partial_duration() {
        let provider = StaticAdProvider::new("https://ads.example.com".to_string(), 10.0);
        let segments = provider.get_ad_segments(25.0, "test-session");

        // 25 / 10 = 2.5, ceiling = 3 segments
        assert_eq!(segments.len(), 3);
    }

    #[test]
    fn test_static_ad_provider_min_one_segment() {
        let provider = StaticAdProvider::new("https://ads.example.com".to_string(), 10.0);
        let segments = provider.get_ad_segments(2.0, "test-session");

        // Even for very short duration, return at least 1 segment
        assert_eq!(segments.len(), 1);
    }

    #[test]
    fn test_static_ad_provider_zero_duration() {
        let provider = StaticAdProvider::new("https://ads.example.com".to_string(), 10.0);
        let segments = provider.get_ad_segments(0.0, "test-session");

        // Should return at least 1 segment
        assert_eq!(segments.len(), 1);
    }

    #[test]
    fn test_parse_segment_index() {
        let provider = StaticAdProvider::new("https://ads.example.com".to_string(), 1.0);
        assert_eq!(provider.parse_segment_index("break-0-seg-0.ts"), Some(0));
        assert_eq!(provider.parse_segment_index("break-0-seg-3.ts"), Some(3));
        assert_eq!(provider.parse_segment_index("break-1-seg-15.ts"), Some(15));
        assert_eq!(provider.parse_segment_index("invalid.ts"), None);
    }

    #[test]
    fn test_resolve_segment_url() {
        let provider = StaticAdProvider::with_segment_count(
            "https://hls.src.tedm.io/content/ts_h264_480p_1s".to_string(),
            1.0,
            10,
        );

        // Test basic resolution
        assert_eq!(
            provider.resolve_segment_url("break-0-seg-0.ts"),
            Some("https://hls.src.tedm.io/content/ts_h264_480p_1s/out_000.ts".to_string())
        );
        assert_eq!(
            provider.resolve_segment_url("break-0-seg-3.ts"),
            Some("https://hls.src.tedm.io/content/ts_h264_480p_1s/out_003.ts".to_string())
        );

        // Test cycling (segment 15 wraps to index 5 with segment_count=10)
        assert_eq!(
            provider.resolve_segment_url("break-1-seg-15.ts"),
            Some("https://hls.src.tedm.io/content/ts_h264_480p_1s/out_005.ts".to_string())
        );

        // Test invalid input
        assert_eq!(provider.resolve_segment_url("invalid.ts"), None);
    }
}
