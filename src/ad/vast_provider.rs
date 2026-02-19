use crate::ad::conditioning;
use crate::ad::provider::{
    AdProvider, AdSegment, AdTrackingInfo, ResolvedSegment, SegmentTrackingContext,
};
use crate::ad::slate::SlateProvider;
use crate::ad::vast::{self, TrackingEvent, VastAdType};
use crate::metrics;
use dashmap::DashMap;
use reqwest::Client;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use tracing::{error, info, warn};

/// Ad creative resolved from VAST (before caching)
#[derive(Debug, Clone)]
struct ResolvedVastCreative {
    /// URL to the ad creative (HLS playlist or MP4)
    url: String,
    /// Duration in seconds
    duration: f32,
    /// Whether this is an HLS stream (vs progressive MP4)
    is_hls: bool,
    /// Impression URLs to fire
    impression_urls: Vec<String>,
    /// Tracking events
    tracking_events: Vec<TrackingEvent>,
    /// Error URL
    error_url: Option<String>,
}

/// Ad creative cached per session with tracking state
#[derive(Debug, Clone)]
#[allow(dead_code)]
struct ResolvedCreative {
    /// URL to the ad creative (HLS playlist or MP4)
    url: String,
    /// Duration in seconds
    duration: f32,
    /// Whether this is an HLS stream (vs progressive MP4)
    is_hls: bool,
    /// Impression URLs to fire
    impression_urls: Vec<String>,
    /// Tracking events
    tracking_events: Vec<TrackingEvent>,
    /// Error URL
    error_url: Option<String>,
    /// Total segments in this ad
    total_segments: usize,
    /// Index of this segment
    segment_index: usize,
    /// Events already fired (for deduplication)
    fired_events: HashSet<String>,
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
    http_client: Client,
    /// Per-session ad cache: maps "session_id:break-N-seg-M" to creative URL
    ad_cache: Arc<DashMap<String, ResolvedCreative>>,
    /// Maximum number of VAST wrapper redirects to follow
    max_wrapper_depth: u32,
    /// VAST request timeout
    timeout: Duration,
    /// Optional slate provider for fallback when VAST returns no ads
    slate: Option<SlateProvider>,
}

impl VastAdProvider {
    /// Create a new VastAdProvider
    ///
    /// # Arguments
    /// * `vast_endpoint` - VAST endpoint URL (supports [DURATION] and [CACHEBUSTING] macros)
    /// * `http_client` - Shared HTTP client for VAST requests
    pub fn new(vast_endpoint: String, http_client: Client) -> Self {
        Self {
            vast_endpoint,
            http_client,
            ad_cache: Arc::new(DashMap::new()),
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
    fn resolve_endpoint(&self, duration: f32) -> String {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();

        self.vast_endpoint
            .replace("[DURATION]", &format!("{}", duration as u32))
            .replace("[CACHEBUSTING]", &format!("{}", timestamp))
    }

    /// Fetch and parse VAST XML, following wrapper chains
    ///
    /// Uses `block_in_place` to run async HTTP requests within the sync
    /// AdProvider trait methods. This is safe because Axum uses a multi-threaded
    /// runtime and `block_in_place` only blocks the current thread.
    ///
    /// Accumulates wrapper tracking data through the chain.
    fn fetch_vast(
        &self,
        url: &str,
        depth: u32,
        session_id: &str,
        wrapper_impressions: &[String],
        wrapper_tracking: &[TrackingEvent],
    ) -> Option<Vec<ResolvedVastCreative>> {
        if depth > self.max_wrapper_depth {
            warn!(
                "VAST wrapper chain exceeded max depth ({})",
                self.max_wrapper_depth
            );
            return None;
        }

        let client = self.http_client.clone();
        let url = url.to_string();
        let timeout = self.timeout;

        // Run async reqwest within sync context using block_in_place
        // Includes 1 retry with 500ms backoff on failure
        let xml = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                let max_attempts = 2;
                for attempt in 1..=max_attempts {
                    let response = client.get(&url).timeout(timeout).send().await;

                    match response {
                        Ok(resp) if resp.status().is_success() => {
                            return resp.text().await.ok();
                        }
                        Ok(resp) => {
                            error!(
                                "VAST endpoint returned status {} (attempt {}/{})",
                                resp.status(),
                                attempt,
                                max_attempts
                            );
                        }
                        Err(e) => {
                            error!(
                                "VAST request failed: {} (attempt {}/{})",
                                e, attempt, max_attempts
                            );
                        }
                    }

                    // Retry backoff (skip on last attempt)
                    if attempt < max_attempts {
                        warn!("Retrying VAST request in 500ms...");
                        tokio::time::sleep(Duration::from_millis(500)).await;
                    }
                }
                None
            })
        })?;

        let vast_response = match vast::parse_vast(&xml) {
            Ok(r) => r,
            Err(e) => {
                error!("Failed to parse VAST XML: {}", e);
                return None;
            }
        };

        let mut creatives = Vec::new();

        for ad in &vast_response.ads {
            match &ad.ad_type {
                VastAdType::InLine(inline) => {
                    for creative in &inline.creatives {
                        if let Some(linear) = &creative.linear
                            && let Some(media_file) =
                                vast::select_best_media_file(&linear.media_files)
                        {
                            // Ad conditioning: check creative compatibility (warnings only)
                            conditioning::check_creative(media_file, session_id);

                            let is_hls = media_file.mime_type == "application/x-mpegURL";

                            // Merge wrapper tracking with inline tracking
                            let mut impression_urls = wrapper_impressions.to_vec();
                            impression_urls.extend(inline.impression_urls.clone());

                            let mut tracking_events = wrapper_tracking.to_vec();
                            tracking_events.extend(linear.tracking_events.clone());

                            creatives.push(ResolvedVastCreative {
                                url: media_file.url.clone(),
                                duration: linear.duration,
                                is_hls,
                                impression_urls,
                                tracking_events,
                                error_url: inline.error_url.clone(),
                            });
                        }
                    }
                }
                VastAdType::Wrapper(wrapper) => {
                    // Accumulate wrapper tracking and follow chain
                    let mut merged_impressions = wrapper_impressions.to_vec();
                    merged_impressions.extend(wrapper.impression_urls.clone());

                    let mut merged_tracking = wrapper_tracking.to_vec();
                    merged_tracking.extend(wrapper.tracking_events.clone());

                    if let Some(mut wrapped_creatives) = self.fetch_vast(
                        &wrapper.ad_tag_uri,
                        depth + 1,
                        session_id,
                        &merged_impressions,
                        &merged_tracking,
                    ) {
                        creatives.append(&mut wrapped_creatives);
                    }
                }
            }
        }

        Some(creatives)
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
            .field("has_slate", &self.slate.is_some())
            .finish()
    }
}

impl AdProvider for VastAdProvider {
    fn get_ad_segments(&self, duration: f32, session_id: &str) -> Vec<AdSegment> {
        let url = self.resolve_endpoint(duration);
        info!(
            "VastAdProvider: Fetching VAST for session {} (duration: {}s) from {}",
            session_id, duration, url
        );

        let creatives = match self.fetch_vast(&url, 0, session_id, &[], &[]) {
            Some(c) if !c.is_empty() => {
                metrics::record_vast_request("success");
                c
            }
            Some(_) => {
                // VAST returned but with no creatives
                metrics::record_vast_request("empty");
                if let Some(slate) = &self.slate {
                    warn!(
                        "VastAdProvider: Empty VAST response for session {} — falling back to slate",
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
                        "VastAdProvider: VAST failed for session {} — falling back to slate",
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
        let break_idx = 0; // TODO: track break index per session
        let total_segments = creatives.len();

        for (seg_idx, creative) in creatives.iter().enumerate() {
            let ad_name = format!("break-{}-seg-{}.ts", break_idx, seg_idx);

            // Cache the resolved creative with tracking metadata
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
                    fired_events: HashSet::new(),
                },
            );

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

    fn resolve_segment_url(&self, ad_name: &str) -> Option<String> {
        // Check if this is a slate segment
        if ad_name.starts_with("slate-seg-") {
            if let Some(slate) = &self.slate {
                return slate.resolve_segment_url(ad_name);
            }
            warn!("VastAdProvider: Slate segment requested but no slate configured");
            return None;
        }

        // Search across all sessions for this ad_name.
        // Ad names include break and segment indices, making them unique enough.
        for entry in self.ad_cache.iter() {
            if entry.key().ends_with(&format!(":{}", ad_name)) {
                return Some(entry.value().url.clone());
            }
        }

        warn!("VastAdProvider: No cached creative found for {}", ad_name);
        None
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
            let tracking = if !entry.fired_events.contains("visited") {
                // Mark as visited
                entry.fired_events.insert("visited".to_string());
                Some(SegmentTrackingContext {
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
}
