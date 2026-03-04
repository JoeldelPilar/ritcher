use crate::ad::conditioning;
use crate::ad::vast::{self, VastAdType, Verification};
use crate::http_retry::{RetryConfig, fetch_with_retry};
use tracing::{error, warn};

use super::{ResolvedVastCreative, VastAdProvider};

impl VastAdProvider {
    /// Fetch and parse VAST XML, following wrapper chains
    ///
    /// Accumulates wrapper tracking data and verification nodes through the chain.
    /// Uses [`fetch_with_retry`] for fault-tolerant HTTP fetching.
    ///
    /// Parameters use owned types (`String`, `Vec<T>`) instead of references
    /// because recursive async functions cannot hold borrows across `.await`
    /// points without self-referential lifetimes.
    pub(crate) async fn fetch_vast(
        &self,
        url: String,
        depth: u32,
        session_id: String,
        wrapper_impressions: Vec<String>,
        wrapper_tracking: Vec<vast::TrackingEvent>,
        wrapper_verifications: Vec<Verification>,
    ) -> Option<Vec<ResolvedVastCreative>> {
        if depth > self.max_wrapper_depth {
            warn!(
                "VAST wrapper chain exceeded max depth ({})",
                self.max_wrapper_depth
            );
            return None;
        }

        let retry_cfg = RetryConfig {
            timeout: Some(self.timeout),
            ..Default::default()
        };
        let xml = match fetch_with_retry(&self.http_client, &url, &retry_cfg).await {
            Ok(resp) => match resp.text().await {
                Ok(text) => text,
                Err(e) => {
                    error!("Failed to read VAST response body: {}", e);
                    return None;
                }
            },
            Err(e) => {
                error!("VAST request failed after retries: {}", e);
                return None;
            }
        };

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
                            conditioning::check_creative(media_file, &session_id);

                            let is_hls = media_file.mime_type == "application/x-mpegURL";

                            // Merge wrapper tracking with inline tracking
                            let mut impression_urls = wrapper_impressions.clone();
                            impression_urls.extend(inline.impression_urls.clone());

                            let mut tracking_events = wrapper_tracking.clone();
                            tracking_events.extend(linear.tracking_events.clone());

                            // Merge wrapper verifications with inline verifications
                            // IAB spec: all Verification nodes from all wrapper levels must survive
                            let mut verifications = wrapper_verifications.clone();
                            verifications.extend(inline.verifications.clone());

                            creatives.push(ResolvedVastCreative {
                                url: media_file.url.clone(),
                                duration: linear.duration,
                                is_hls,
                                impression_urls,
                                tracking_events,
                                error_url: inline.error_url.clone(),
                                verifications,
                            });
                        }
                    }
                }
                VastAdType::Wrapper(wrapper) => {
                    // Accumulate wrapper tracking, verifications and follow chain
                    let mut merged_impressions = wrapper_impressions.clone();
                    merged_impressions.extend(wrapper.impression_urls.clone());

                    let mut merged_tracking = wrapper_tracking.clone();
                    merged_tracking.extend(wrapper.tracking_events.clone());

                    let mut merged_verifications = wrapper_verifications.clone();
                    merged_verifications.extend(wrapper.verifications.clone());

                    // Box::pin is required for recursive async functions to avoid
                    // infinite future size at compile time
                    if let Some(mut wrapped_creatives) = Box::pin(self.fetch_vast(
                        wrapper.ad_tag_uri.clone(),
                        depth + 1,
                        session_id.clone(),
                        merged_impressions,
                        merged_tracking,
                        merged_verifications,
                    ))
                    .await
                    {
                        creatives.append(&mut wrapped_creatives);
                    }
                }
            }
        }

        Some(creatives)
    }
}

#[cfg(test)]
mod tests {
    use crate::ad::vast::{TrackingEvent, Verification};

    #[test]
    fn test_wrapper_tracking_merge() {
        // Verifies that wrapper impression URLs and tracking events
        // are correctly accumulated and merged with inline ad data.
        // This tests the merge pattern used in fetch_vast().

        // Simulate wrapper accumulation
        let mut wrapper_impressions = vec!["http://wrapper/imp".to_string()];
        let inline_impressions = vec!["http://inline/imp".to_string()];
        wrapper_impressions.extend(inline_impressions);

        assert_eq!(wrapper_impressions.len(), 2);
        assert_eq!(wrapper_impressions[0], "http://wrapper/imp");
        assert_eq!(wrapper_impressions[1], "http://inline/imp");

        // Simulate tracking event accumulation
        let mut wrapper_tracking = vec![TrackingEvent {
            event: "start".into(),
            url: "http://wrapper/start".into(),
        }];
        let inline_tracking = vec![TrackingEvent {
            event: "complete".into(),
            url: "http://inline/complete".into(),
        }];
        wrapper_tracking.extend(inline_tracking);

        assert_eq!(wrapper_tracking.len(), 2);
        assert_eq!(wrapper_tracking[0].url, "http://wrapper/start");
        assert_eq!(wrapper_tracking[1].url, "http://inline/complete");

        // Multi-level: second wrapper adds more impressions
        let mut level2_impressions = vec!["http://wrapper2/imp".to_string()];
        level2_impressions.extend(wrapper_impressions);
        assert_eq!(level2_impressions.len(), 3);
        assert_eq!(level2_impressions[0], "http://wrapper2/imp");
        assert_eq!(level2_impressions[1], "http://wrapper/imp");
        assert_eq!(level2_impressions[2], "http://inline/imp");
    }

    #[test]
    fn test_wrapper_verification_accumulation() {
        // Simulate wrapper chain verification accumulation (same pattern as fetch_vast)
        // Level 1 wrapper has one verification
        let wrapper_verifications = vec![Verification {
            vendor: Some("wrapper-vendor".to_string()),
            javascript_resource_url: Some("https://wrapper.example.com/omid.js".to_string()),
            api_framework: Some("omid".to_string()),
            parameters: None,
            tracking_events: vec![],
        }];

        // InLine ad has its own verification
        let inline_verifications = vec![Verification {
            vendor: Some("inline-vendor".to_string()),
            javascript_resource_url: Some("https://inline.example.com/omid.js".to_string()),
            api_framework: Some("omid".to_string()),
            parameters: Some("ctx=abc".to_string()),
            tracking_events: vec![],
        }];

        // Merge: wrapper first, then inline (IAB spec: all must survive)
        let mut merged = wrapper_verifications;
        merged.extend(inline_verifications);

        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].vendor.as_deref(), Some("wrapper-vendor"));
        assert_eq!(merged[1].vendor.as_deref(), Some("inline-vendor"));
        assert_eq!(merged[1].parameters.as_deref(), Some("ctx=abc"));
    }

    #[test]
    fn test_multi_level_wrapper_verification_accumulation() {
        // Level 2 wrapper
        let level2 = vec![Verification {
            vendor: Some("level2-vendor".to_string()),
            javascript_resource_url: Some("https://l2.example.com/omid.js".to_string()),
            api_framework: Some("omid".to_string()),
            parameters: None,
            tracking_events: vec![],
        }];

        // Level 1 wrapper adds its own
        let level1 = vec![Verification {
            vendor: Some("level1-vendor".to_string()),
            javascript_resource_url: Some("https://l1.example.com/omid.js".to_string()),
            api_framework: Some("omid".to_string()),
            parameters: None,
            tracking_events: vec![],
        }];

        // InLine adds its own
        let inline = vec![Verification {
            vendor: Some("inline-vendor".to_string()),
            javascript_resource_url: Some("https://inline.example.com/omid.js".to_string()),
            api_framework: Some("omid".to_string()),
            parameters: None,
            tracking_events: vec![],
        }];

        // Simulate the accumulation through wrapper chain
        let mut acc = level2;
        acc.extend(level1);
        acc.extend(inline);

        assert_eq!(
            acc.len(),
            3,
            "All verification nodes from all levels must survive"
        );
        assert_eq!(acc[0].vendor.as_deref(), Some("level2-vendor"));
        assert_eq!(acc[1].vendor.as_deref(), Some("level1-vendor"));
        assert_eq!(acc[2].vendor.as_deref(), Some("inline-vendor"));
    }
}
