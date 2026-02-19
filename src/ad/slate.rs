use crate::ad::provider::{AdProvider, AdSegment};
use tracing::info;

/// Slate provider for fallback content during ad breaks
///
/// When the primary ad provider (VAST) returns no ads or fails,
/// the slate provider fills the remaining duration with looping
/// filler segments from a configured slate URL.
///
/// The slate is typically a short looping video ("We'll be right back",
/// channel branding, etc.) that cycles to fill any duration.
#[derive(Clone, Debug)]
pub struct SlateProvider {
    /// Base URL for slate segments (HLS source with .ts segments)
    slate_url: String,
    /// Duration of each slate segment in seconds
    segment_duration: f32,
    /// Number of available segments in the slate source (for cycling)
    segment_count: usize,
}

impl SlateProvider {
    /// Create a new SlateProvider
    ///
    /// # Arguments
    /// * `slate_url` - Base URL where slate segments are hosted
    /// * `segment_duration` - Duration of each slate segment in seconds
    pub fn new(slate_url: String, segment_duration: f32) -> Self {
        Self {
            slate_url,
            segment_duration,
            segment_count: 10, // Default, same as static provider
        }
    }

    /// Generate slate segments to fill the given duration
    ///
    /// Used directly by VastAdProvider when VAST returns empty or fails.
    /// Cycles through available slate segments to fill the requested duration.
    pub fn fill_duration(&self, duration: f32, session_id: &str) -> Vec<AdSegment> {
        let num_segments = (duration / self.segment_duration).ceil() as usize;
        let num_segments = num_segments.max(1);

        info!(
            "SlateProvider: Generating {} slate segments for session {} (duration: {}s)",
            num_segments, session_id, duration
        );

        (0..num_segments)
            .map(|i| AdSegment {
                uri: format!("slate-seg-{}.ts", i),
                duration: self.segment_duration,
                tracking: None,
            })
            .collect()
    }

    /// Resolve a slate segment identifier to its actual source URL
    ///
    /// Slate segments use the naming format "slate-seg-{index}.ts"
    pub fn resolve_segment_url(&self, segment_name: &str) -> Option<String> {
        let index = segment_name
            .strip_prefix("slate-seg-")
            .and_then(|s| s.strip_suffix(".ts"))
            .and_then(|s| s.parse::<usize>().ok())?;

        let source_index = index % self.segment_count;
        let source_segment = format!("out_{:03}.ts", source_index);

        Some(format!("{}/{}", self.slate_url, source_segment))
    }
}

/// Standalone AdProvider implementation for slate-only mode
///
/// Used when no VAST endpoint is configured and the operator wants
/// to serve slate content for all ad breaks. Also useful for testing.
impl AdProvider for SlateProvider {
    fn get_ad_segments(&self, duration: f32, session_id: &str) -> Vec<AdSegment> {
        self.fill_duration(duration, session_id)
    }

    fn resolve_segment_url(&self, ad_name: &str) -> Option<String> {
        self.resolve_segment_url(ad_name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fill_duration_exact() {
        let provider = SlateProvider::new("https://slate.example.com".to_string(), 2.0);
        let segments = provider.fill_duration(10.0, "test-session");

        assert_eq!(segments.len(), 5);
        for (i, seg) in segments.iter().enumerate() {
            assert_eq!(seg.uri, format!("slate-seg-{}.ts", i));
            assert_eq!(seg.duration, 2.0);
            assert_eq!(seg.tracking, None);
        }
    }

    #[test]
    fn test_fill_duration_partial() {
        let provider = SlateProvider::new("https://slate.example.com".to_string(), 2.0);
        let segments = provider.fill_duration(7.0, "test-session");

        // 7.0 / 2.0 = 3.5, ceil = 4
        assert_eq!(segments.len(), 4);
    }

    #[test]
    fn test_fill_duration_minimum_one() {
        let provider = SlateProvider::new("https://slate.example.com".to_string(), 10.0);
        let segments = provider.fill_duration(0.0, "test-session");

        assert_eq!(segments.len(), 1);
    }

    #[test]
    fn test_resolve_segment_url() {
        let provider = SlateProvider::new("https://slate.example.com/content".to_string(), 1.0);

        assert_eq!(
            provider.resolve_segment_url("slate-seg-0.ts"),
            Some("https://slate.example.com/content/out_000.ts".to_string())
        );
        assert_eq!(
            provider.resolve_segment_url("slate-seg-3.ts"),
            Some("https://slate.example.com/content/out_003.ts".to_string())
        );
        // Cycling: index 15 wraps to 5 with segment_count=10
        assert_eq!(
            provider.resolve_segment_url("slate-seg-15.ts"),
            Some("https://slate.example.com/content/out_005.ts".to_string())
        );
    }

    #[test]
    fn test_resolve_segment_url_invalid() {
        let provider = SlateProvider::new("https://slate.example.com".to_string(), 1.0);

        assert_eq!(provider.resolve_segment_url("invalid.ts"), None);
        assert_eq!(provider.resolve_segment_url("break-0-seg-0.ts"), None);
    }

    #[test]
    fn test_ad_provider_trait() {
        let provider = SlateProvider::new("https://slate.example.com".to_string(), 2.0);

        // Test via AdProvider trait
        let segments = provider.get_ad_segments(6.0, "session-1");
        assert_eq!(segments.len(), 3);

        let url = AdProvider::resolve_segment_url(&provider, "slate-seg-0.ts");
        assert!(url.is_some());
    }
}
