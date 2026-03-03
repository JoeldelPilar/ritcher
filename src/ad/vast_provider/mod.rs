mod cache;
mod fetch;

use crate::ad::provider::{AdCreative, AdProvider, AdSegment, AdTrackingInfo, ResolvedSegment};
use crate::ad::slate::SlateProvider;
use crate::ad::vast::{TrackingEvent, Verification};
use crate::metrics;
use async_trait::async_trait;
use cache::MAX_CACHE_SIZE;
use dashmap::DashMap;
use reqwest::Client;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{info, warn};

/// Ad creative resolved from VAST (before caching)
#[derive(Debug, Clone)]
pub(crate) struct ResolvedVastCreative {
    /// URL to the ad creative (HLS playlist or MP4)
    pub(crate) url: String,
    /// Duration in seconds
    pub(crate) duration: f32,
    /// Whether this is an HLS stream (vs progressive MP4)
    pub(crate) is_hls: bool,
    /// Impression URLs to fire
    pub(crate) impression_urls: Vec<String>,
    /// Tracking events
    pub(crate) tracking_events: Vec<TrackingEvent>,
    /// Error URL
    pub(crate) error_url: Option<String>,
    /// OMID verification resources accumulated from wrapper chain + InLine
    pub(crate) verifications: Vec<Verification>,
}

/// Ad creative cached per session with tracking state
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct ResolvedCreative {
    /// URL to the ad creative (HLS playlist or MP4)
    pub(crate) url: String,
    /// Duration in seconds
    pub(crate) duration: f32,
    /// Whether this is an HLS stream (vs progressive MP4)
    pub(crate) is_hls: bool,
    /// Impression URLs to fire
    pub(crate) impression_urls: Vec<String>,
    /// Tracking events
    pub(crate) tracking_events: Vec<TrackingEvent>,
    /// Error URL
    pub(crate) error_url: Option<String>,
    /// Total segments in this ad
    pub(crate) total_segments: usize,
    /// Index of this segment
    pub(crate) segment_index: usize,
    /// Whether tracking has been returned for this segment (deduplication)
    pub(crate) visited: bool,
    /// When this entry was inserted (for TTL-based eviction)
    pub(crate) inserted_at: Instant,
}

/// VAST-based ad provider that fetches ads from a VAST endpoint
///
/// Implements the AdProvider trait by:
/// 1. Fetching VAST XML from configured endpoint on each ad break
/// 2. Parsing the response to extract media file URLs and durations
/// 3. Caching resolved creatives per session for segment URL resolution
#[derive(Clone)]
pub struct VastAdProvider {
    /// VAST endpoint URL (with optional macros like [DURATION])
    vast_endpoint: String,
    /// HTTP client for VAST requests
    pub(crate) http_client: Client,
    /// Per-session ad cache: maps "session_id:break-N-seg-M" to creative URL
    pub(crate) ad_cache: Arc<DashMap<String, ResolvedCreative>>,
    /// Per-session break counter: tracks next break index for each session
    pub(crate) break_counter: Arc<DashMap<String, u32>>,
    /// Maximum number of VAST wrapper redirects to follow
    pub(crate) max_wrapper_depth: u32,
    /// VAST request timeout
    pub(crate) timeout: Duration,
    /// Optional slate provider for fallback when VAST returns no ads
    pub(crate) slate: Option<SlateProvider>,
}

impl VastAdProvider {
    /// Create a new VastAdProvider
    ///
    /// # Arguments
    /// * `vast_endpoint` - VAST endpoint URL (supports `[DURATION]` and `[CACHEBUSTING]` macros)
    /// * `http_client` - Shared HTTP client for VAST requests
    pub fn new(vast_endpoint: String, http_client: Client) -> Self {
        Self {
            vast_endpoint,
            http_client,
            ad_cache: Arc::new(DashMap::new()),
            break_counter: Arc::new(DashMap::new()),
            max_wrapper_depth: 5,
            timeout: Duration::from_millis(2000),
            slate: None,
        }
    }

    /// Configure a slate provider for fallback when VAST returns no ads
    ///
    /// When set, empty VAST responses or VAST failures will fall back to
    /// serving slate segments instead of returning an empty ad break.
    pub fn with_slate(mut self, slate: SlateProvider) -> Self {
        self.slate = Some(slate);
        self
    }

    /// Replace VAST macros in the endpoint URL
    pub(crate) fn resolve_endpoint(&self, duration: f32) -> String {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();

        // VAST macro: duration is always a positive f32 representing seconds;
        // truncation to u32 is intentional (ad servers expect integer seconds).
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let dur_secs = duration as u32;
        self.vast_endpoint
            .replace("[DURATION]", &format!("{}", dur_secs))
            .replace("[CACHEBUSTING]", &format!("{}", timestamp))
    }

    /// Generate slate fallback segments when VAST returns no ads
    ///
    /// Slate segments use "slate-seg-N.ts" naming to distinguish them
    /// from regular VAST ad segments ("break-N-seg-M.ts").
    fn slate_fallback(
        &self,
        slate: &SlateProvider,
        duration: f32,
        session_id: &str,
    ) -> Vec<AdSegment> {
        let segments = slate.fill_duration(duration, session_id);
        info!(
            "VastAdProvider: Slate fallback generated {} segments for session {}",
            segments.len(),
            session_id
        );
        segments
    }

    /// Build cache key for ad segment lookup
    fn cache_key(session_id: &str, ad_name: &str) -> String {
        format!("{}:{}", session_id, ad_name)
    }
}

impl std::fmt::Debug for VastAdProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VastAdProvider")
            .field("vast_endpoint", &self.vast_endpoint)
            .field("max_wrapper_depth", &self.max_wrapper_depth)
            .field("timeout", &self.timeout)
            .field("cached_entries", &self.ad_cache.len())
            .field("active_sessions", &self.break_counter.len())
            .field("has_slate", &self.slate.is_some())
            .finish()
    }
}

#[async_trait]
impl AdProvider for VastAdProvider {
    async fn get_ad_segments(&self, duration: f32, session_id: &str) -> Vec<AdSegment> {
        let url = self.resolve_endpoint(duration);
        info!(
            "VastAdProvider: Fetching VAST for session {} (duration: {}s) from {}",
            session_id, duration, url
        );

        let creatives = match self
            .fetch_vast(url, 0, session_id.to_string(), vec![], vec![], vec![])
            .await
        {
            Some(c) if !c.is_empty() => {
                metrics::record_vast_request("success");
                c
            }
            Some(_) => {
                // VAST returned but with no creatives
                metrics::record_vast_request("empty");
                if let Some(slate) = &self.slate {
                    warn!(
                        "VastAdProvider: Empty VAST response for session {} \u{2014} falling back to slate",
                        session_id
                    );
                    metrics::record_slate_fallback();
                    return self.slate_fallback(slate, duration, session_id);
                }
                warn!(
                    "VastAdProvider: Empty VAST response for session {} and no slate configured",
                    session_id
                );
                return Vec::new();
            }
            None => {
                // VAST request failed
                metrics::record_vast_request("error");
                if let Some(slate) = &self.slate {
                    warn!(
                        "VastAdProvider: VAST failed for session {} \u{2014} falling back to slate",
                        session_id
                    );
                    metrics::record_slate_fallback();
                    return self.slate_fallback(slate, duration, session_id);
                }
                warn!(
                    "VastAdProvider: VAST failed for session {} and no slate configured",
                    session_id
                );
                return Vec::new();
            }
        };

        // Build ad segments and cache them for resolve_segment_url
        let mut segments = Vec::new();
        let break_idx = {
            let mut counter = self
                .break_counter
                .entry(session_id.to_string())
                .or_insert(0);
            let idx = *counter;
            *counter += 1;
            idx
        };
        let total_segments = creatives.len();

        for (seg_idx, creative) in creatives.iter().enumerate() {
            let ad_name = format!("break-{}-seg-{}.ts", break_idx, seg_idx);

            // Cache the resolved creative with tracking metadata.
            // Guard against unbounded growth between cleanup cycles.
            if self.ad_cache.len() >= MAX_CACHE_SIZE {
                warn!(
                    "Ad cache at capacity ({} entries) \u{2014} skipping insert for {}",
                    MAX_CACHE_SIZE, ad_name
                );
            } else {
                self.ad_cache.insert(
                    Self::cache_key(session_id, &ad_name),
                    ResolvedCreative {
                        url: creative.url.clone(),
                        duration: creative.duration,
                        is_hls: creative.is_hls,
                        impression_urls: creative.impression_urls.clone(),
                        tracking_events: creative.tracking_events.clone(),
                        error_url: creative.error_url.clone(),
                        total_segments,
                        segment_index: seg_idx,
                        visited: false,
                        inserted_at: Instant::now(),
                    },
                );
            }

            segments.push(AdSegment {
                uri: ad_name,
                duration: creative.duration,
                tracking: Some(AdTrackingInfo {
                    impression_urls: creative.impression_urls.clone(),
                    tracking_events: creative.tracking_events.clone(),
                    error_url: creative.error_url.clone(),
                    total_segments,
                    segment_index: seg_idx,
                }),
            });
        }

        info!(
            "VastAdProvider: Resolved {} ad segment(s) for session {}",
            segments.len(),
            session_id
        );

        segments
    }

    fn resolve_segment_url(&self, ad_name: &str, session_id: &str) -> Option<String> {
        // Check if this is a slate segment
        if ad_name.starts_with("slate-seg-") {
            if let Some(slate) = &self.slate {
                return slate.resolve_segment_url(ad_name);
            }
            warn!("VastAdProvider: Slate segment requested but no slate configured");
            return None;
        }

        // Direct O(1) cache lookup using session_id + ad_name
        let cache_key = Self::cache_key(session_id, ad_name);
        if let Some(entry) = self.ad_cache.get(&cache_key) {
            return Some(entry.url.clone());
        }

        warn!("VastAdProvider: No cached creative found for {}", ad_name);
        None
    }

    async fn get_ad_creatives(&self, duration: f32, session_id: &str) -> Vec<AdCreative> {
        let url = self.resolve_endpoint(duration);
        info!(
            "VastAdProvider: Fetching VAST creatives for session {} (duration: {}s)",
            session_id, duration
        );

        match self
            .fetch_vast(url, 0, session_id.to_string(), vec![], vec![], vec![])
            .await
        {
            Some(creatives) if !creatives.is_empty() => {
                metrics::record_vast_request("success");
                creatives
                    .into_iter()
                    .map(|c| AdCreative {
                        uri: c.url,
                        duration: c.duration as f64,
                        verifications: c.verifications,
                    })
                    .collect()
            }
            Some(_) => {
                metrics::record_vast_request("empty");
                warn!(
                    "VastAdProvider: Empty VAST response for session {} (get_ad_creatives)",
                    session_id
                );
                Vec::new()
            }
            None => {
                metrics::record_vast_request("error");
                warn!(
                    "VastAdProvider: VAST failed for session {} (get_ad_creatives)",
                    session_id
                );
                Vec::new()
            }
        }
    }

    fn cleanup_cache(&self) {
        self.run_cleanup_cache();
    }

    fn resolve_segment_with_tracking(
        &self,
        ad_name: &str,
        session_id: &str,
    ) -> Option<ResolvedSegment> {
        // Slate segments have no tracking
        if ad_name.starts_with("slate-seg-") {
            if let Some(slate) = &self.slate {
                return slate
                    .resolve_segment_url(ad_name)
                    .map(|url| ResolvedSegment {
                        url,
                        tracking: None,
                    });
            }
            return None;
        }

        let cache_key = Self::cache_key(session_id, ad_name);
        if let Some(mut entry) = self.ad_cache.get_mut(&cache_key) {
            // Check if this segment has been visited (deduplication)
            let tracking = if !entry.visited {
                // Mark as visited
                entry.visited = true;
                Some(AdTrackingInfo {
                    impression_urls: entry.impression_urls.clone(),
                    tracking_events: entry.tracking_events.clone(),
                    error_url: entry.error_url.clone(),
                    total_segments: entry.total_segments,
                    segment_index: entry.segment_index,
                })
            } else {
                // Already served, don't fire tracking again
                None
            };

            Some(ResolvedSegment {
                url: entry.url.clone(),
                tracking,
            })
        } else {
            warn!("VastAdProvider: No cached creative found for {}", ad_name);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_endpoint_macros() {
        let client = Client::new();
        let provider = VastAdProvider::new(
            "http://ads.example.com/vast?dur=[DURATION]&cb=[CACHEBUSTING]".to_string(),
            client,
        );

        let resolved = provider.resolve_endpoint(30.0);
        assert!(resolved.contains("dur=30"));
        assert!(!resolved.contains("[CACHEBUSTING]"));
        assert!(!resolved.contains("[DURATION]"));
    }

    #[test]
    fn test_cache_key() {
        assert_eq!(
            VastAdProvider::cache_key("session-1", "break-0-seg-0.ts"),
            "session-1:break-0-seg-0.ts"
        );
    }

    #[test]
    fn resolve_segment_url_finds_cached_entry() {
        let client = Client::new();
        let provider = VastAdProvider::new("http://ads.example.com/vast".to_string(), client);

        provider.ad_cache.insert(
            "session-1:break-0-seg-0.ts".to_string(),
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

        assert_eq!(
            provider.resolve_segment_url("break-0-seg-0.ts", "session-1"),
            Some("http://cdn.example.com/ad.m3u8".to_string())
        );
    }

    #[test]
    fn resolve_segment_url_returns_none_for_unknown() {
        let client = Client::new();
        let provider = VastAdProvider::new("http://ads.example.com/vast".to_string(), client);
        assert!(
            provider
                .resolve_segment_url("break-0-seg-99.ts", "session-1")
                .is_none()
        );
    }

    #[test]
    fn resolve_segment_with_tracking_dedup() {
        let client = Client::new();
        let provider = VastAdProvider::new("http://ads.example.com/vast".to_string(), client);

        provider.ad_cache.insert(
            "session-x:break-0-seg-0.ts".to_string(),
            ResolvedCreative {
                url: "http://cdn.example.com/ad.m3u8".to_string(),
                duration: 15.0,
                is_hls: true,
                impression_urls: vec!["http://impression.example.com".to_string()],
                tracking_events: vec![],
                error_url: None,
                total_segments: 1,
                segment_index: 0,
                visited: false,
                inserted_at: Instant::now(),
            },
        );

        // First access -- tracking should be returned
        let result1 = provider.resolve_segment_with_tracking("break-0-seg-0.ts", "session-x");
        assert!(result1.is_some());
        assert!(
            result1.unwrap().tracking.is_some(),
            "First access should return tracking"
        );

        // Second access -- tracking suppressed (dedup via visited flag)
        let result2 = provider.resolve_segment_with_tracking("break-0-seg-0.ts", "session-x");
        assert!(result2.is_some());
        assert!(
            result2.unwrap().tracking.is_none(),
            "Second access should not return tracking (dedup)"
        );
    }

    #[test]
    fn with_slate_configures_slate_provider() {
        use crate::ad::slate::SlateProvider;

        let client = Client::new();
        let provider = VastAdProvider::new("http://ads.example.com/vast".to_string(), client);
        assert!(provider.slate.is_none(), "No slate by default");

        let slate = SlateProvider::new("http://slate.example.com".to_string(), 1.0);
        let provider = provider.with_slate(slate);
        assert!(
            provider.slate.is_some(),
            "Slate should be configured after with_slate()"
        );
    }

    #[tokio::test]
    async fn get_ad_segments_fetches_vast_and_caches() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        // Minimal VAST 3.0 inline with one HLS creative (matches vast.rs test fixture format)
        const VAST_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<VAST version="3.0">
  <Ad id="ad-001">
    <InLine>
      <AdSystem>TestAds</AdSystem>
      <AdTitle>Test Ad</AdTitle>
      <Impression>http://impression.example.com/track</Impression>
      <Creatives>
        <Creative id="creative-001">
          <Linear>
            <Duration>00:00:15</Duration>
            <MediaFiles>
              <MediaFile delivery="streaming" type="application/x-mpegURL" width="1280" height="720">
                http://ad.example.com/ad.m3u8
              </MediaFile>
            </MediaFiles>
          </Linear>
        </Creative>
      </Creatives>
    </InLine>
  </Ad>
</VAST>"#;

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string(VAST_XML))
            .mount(&server)
            .await;

        let client = Client::new();
        let provider = VastAdProvider::new(server.uri(), client);

        let segments = provider.get_ad_segments(30.0, "session-vast").await;

        assert!(!segments.is_empty(), "Should return ad segments from VAST");
        assert_eq!(
            segments[0].duration, 15.0,
            "Duration should match VAST response"
        );
        assert_eq!(segments[0].uri, "break-0-seg-0.ts");

        // Verify the resolved creative was cached so segment URLs can be resolved
        let cached_url = provider.resolve_segment_url("break-0-seg-0.ts", "session-vast");
        assert_eq!(
            cached_url,
            Some("http://ad.example.com/ad.m3u8".to_string()),
            "Resolved creative should be cached after get_ad_segments"
        );
    }

    #[tokio::test]
    async fn get_ad_segments_multi_break_uses_unique_indices() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        const VAST_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<VAST version="3.0">
  <Ad id="ad-001">
    <InLine>
      <AdSystem>TestAds</AdSystem>
      <AdTitle>Test Ad</AdTitle>
      <Impression>http://impression.example.com/track</Impression>
      <Creatives>
        <Creative id="creative-001">
          <Linear>
            <Duration>00:00:15</Duration>
            <MediaFiles>
              <MediaFile delivery="streaming" type="application/x-mpegURL" width="1280" height="720">
                http://ad.example.com/ad.m3u8
              </MediaFile>
            </MediaFiles>
          </Linear>
        </Creative>
      </Creatives>
    </InLine>
  </Ad>
</VAST>"#;

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string(VAST_XML))
            .mount(&server)
            .await;

        let client = Client::new();
        let provider = VastAdProvider::new(server.uri(), client);

        // First ad break for this session
        let segments1 = provider.get_ad_segments(30.0, "session-multi").await;
        assert_eq!(segments1[0].uri, "break-0-seg-0.ts");

        // Second ad break for same session -- should get break-1, not break-0
        let segments2 = provider.get_ad_segments(30.0, "session-multi").await;
        assert_eq!(
            segments2[0].uri, "break-1-seg-0.ts",
            "Second break should use break_idx=1, not overwrite break-0"
        );

        // Both should be independently cached
        assert!(
            provider
                .ad_cache
                .contains_key("session-multi:break-0-seg-0.ts")
        );
        assert!(
            provider
                .ad_cache
                .contains_key("session-multi:break-1-seg-0.ts")
        );

        // Different session should start at break-0
        let segments3 = provider.get_ad_segments(30.0, "session-other").await;
        assert_eq!(
            segments3[0].uri, "break-0-seg-0.ts",
            "Different session should start at break-0"
        );
    }

    #[tokio::test]
    async fn get_ad_creatives_includes_verifications() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        const VAST_WITH_OMID: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<VAST version="4.1">
  <Ad id="omid-ad">
    <InLine>
      <AdSystem>TestAds</AdSystem>
      <AdTitle>OMID Ad</AdTitle>
      <Impression>http://impression.example.com/track</Impression>
      <AdVerifications>
        <Verification vendor="doubleverify.com-omid" apiFramework="omid">
          <JavaScriptResource>https://cdn.dv.com/dvtp_src.js</JavaScriptResource>
          <VerificationParameters>ctx=123</VerificationParameters>
        </Verification>
      </AdVerifications>
      <Creatives>
        <Creative id="c1">
          <Linear>
            <Duration>00:00:15</Duration>
            <MediaFiles>
              <MediaFile delivery="streaming" type="application/x-mpegURL" width="1280" height="720">
                http://ad.example.com/ad.m3u8
              </MediaFile>
            </MediaFiles>
          </Linear>
        </Creative>
      </Creatives>
    </InLine>
  </Ad>
</VAST>"#;

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string(VAST_WITH_OMID))
            .mount(&server)
            .await;

        let client = Client::new();
        let provider = VastAdProvider::new(server.uri(), client);

        let creatives = provider.get_ad_creatives(30.0, "session-omid").await;

        assert!(!creatives.is_empty(), "Should return creatives");
        assert_eq!(
            creatives[0].verifications.len(),
            1,
            "Creative should carry verification data"
        );
        assert_eq!(
            creatives[0].verifications[0].vendor.as_deref(),
            Some("doubleverify.com-omid")
        );
        assert_eq!(
            creatives[0].verifications[0]
                .javascript_resource_url
                .as_deref(),
            Some("https://cdn.dv.com/dvtp_src.js")
        );
        assert_eq!(
            creatives[0].verifications[0].parameters.as_deref(),
            Some("ctx=123")
        );
    }

    #[tokio::test]
    async fn get_ad_creatives_without_verifications() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        // Standard VAST without <AdVerifications>
        const VAST_NO_OMID: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<VAST version="3.0">
  <Ad id="ad-001">
    <InLine>
      <AdSystem>TestAds</AdSystem>
      <AdTitle>Test Ad</AdTitle>
      <Impression>http://impression.example.com/track</Impression>
      <Creatives>
        <Creative id="c1">
          <Linear>
            <Duration>00:00:15</Duration>
            <MediaFiles>
              <MediaFile delivery="streaming" type="application/x-mpegURL" width="1280" height="720">
                http://ad.example.com/ad.m3u8
              </MediaFile>
            </MediaFiles>
          </Linear>
        </Creative>
      </Creatives>
    </InLine>
  </Ad>
</VAST>"#;

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string(VAST_NO_OMID))
            .mount(&server)
            .await;

        let client = Client::new();
        let provider = VastAdProvider::new(server.uri(), client);

        let creatives = provider.get_ad_creatives(30.0, "session-no-omid").await;

        assert!(!creatives.is_empty());
        assert!(
            creatives[0].verifications.is_empty(),
            "No verifications when VAST has no AdVerifications"
        );
    }
}
