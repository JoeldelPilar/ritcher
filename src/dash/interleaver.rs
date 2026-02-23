use crate::ad::provider::AdSegment;
use crate::dash::cue::DashAdBreak;
use dash_mpd::{AdaptationSet, MPD, Period, Representation, SegmentList, SegmentURL};
use std::time::Duration;
use tracing::{info, warn};

/// Interleave ad segments into DASH MPD by inserting ad Periods
///
/// Creates new Period elements with SegmentList-based ad content and inserts them
/// after the Periods containing ad break signals (detected by DashAdBreak).
///
/// Ad Periods mirror the content Period's AdaptationSet structure (video, audio, etc.)
/// so that all tracks are present during ad breaks. Since ad creatives are typically
/// muxed (containing both audio and video), the same SegmentList URLs are used for
/// all AdaptationSets — the player demuxes the correct track.
///
/// # Arguments
/// * `mpd` - The original MPD to modify
/// * `ad_breaks` - Detected ad breaks from EventStream/SCTE-35
/// * `ad_segments` - Ad segments to insert (one Vec per ad break)
/// * `session_id` - Session ID for URL generation
/// * `base_url` - Stitcher base URL for proxying
///
/// # Returns
/// Modified MPD with ad Periods inserted
pub fn interleave_ads_mpd(
    mut mpd: MPD,
    ad_breaks: &[DashAdBreak],
    ad_segments_per_break: &[Vec<AdSegment>],
    session_id: &str,
    base_url: &str,
) -> MPD {
    if ad_breaks.is_empty() {
        info!("No ad breaks detected, returning MPD unchanged");
        return mpd;
    }

    if ad_breaks.len() != ad_segments_per_break.len() {
        warn!(
            "Mismatch between ad breaks ({}) and ad segment sets ({})",
            ad_breaks.len(),
            ad_segments_per_break.len()
        );
        return mpd;
    }

    // Iterate ad breaks in reverse order to preserve period indices when inserting
    for (break_idx, ad_break) in ad_breaks.iter().enumerate().rev() {
        let ad_segments = &ad_segments_per_break[break_idx];

        if ad_segments.is_empty() {
            warn!("Ad break {} has no segments, skipping", break_idx);
            continue;
        }

        // Get content AdaptationSets from the signal Period to mirror in ad Period
        let content_adaptations = mpd
            .periods
            .get(ad_break.period_index)
            .map(|p| p.adaptations.as_slice())
            .unwrap_or(&[]);

        info!(
            "Inserting {} ad segments at Period {} (ad break {}/{}, {} content AdaptationSets)",
            ad_segments.len(),
            ad_break.period_index,
            break_idx + 1,
            ad_breaks.len(),
            content_adaptations.len()
        );

        // Create ad Period mirroring content track structure
        let ad_period = create_ad_period(
            ad_segments,
            break_idx,
            session_id,
            base_url,
            content_adaptations,
        );

        // Insert ad Period after the signal period
        let insert_position = ad_break.period_index + 1;
        if insert_position <= mpd.periods.len() {
            mpd.periods.insert(insert_position, ad_period);
        } else {
            warn!(
                "Invalid period index {} for ad break {}, appending at end",
                ad_break.period_index, break_idx
            );
            mpd.periods.push(ad_period);
        }
    }

    info!(
        "Interleaving complete: MPD now has {} periods ({} ad breaks inserted)",
        mpd.periods.len(),
        ad_breaks.len()
    );

    mpd
}

/// Create a DASH Period containing ad content with SegmentList
///
/// Mirrors the content Period's AdaptationSet structure so that all tracks
/// (video, audio, etc.) are present in the ad Period. Since ad creatives are
/// typically muxed, the same SegmentList URLs are shared across all tracks.
///
/// Falls back to a single video-only AdaptationSet when no content AdaptationSets
/// are available (backward compatibility).
///
/// # Arguments
/// * `ad_segments` - Ad segments to include in this Period
/// * `break_idx` - Index of this ad break (for ID generation)
/// * `session_id` - Session ID for URL generation
/// * `base_url` - Stitcher base URL for proxying
/// * `content_adaptations` - AdaptationSets from the content Period to mirror
///
/// # Returns
/// A Period with ad content matching the content track structure
fn create_ad_period(
    ad_segments: &[AdSegment],
    break_idx: usize,
    session_id: &str,
    base_url: &str,
    content_adaptations: &[AdaptationSet],
) -> Period {
    // Calculate total duration
    let total_duration: f64 = ad_segments.iter().map(|s| s.duration as f64).sum();

    // Create SegmentURL entries for each ad segment (shared across all tracks)
    let segment_urls: Vec<SegmentURL> = ad_segments
        .iter()
        .enumerate()
        .map(|(seg_idx, _seg)| SegmentURL {
            media: Some(format!(
                "{}/stitch/{}/ad/break-{}-seg-{}.ts",
                base_url, session_id, break_idx, seg_idx
            )),
            ..Default::default()
        })
        .collect();

    // Mirror content AdaptationSets, or fall back to single video
    let adaptations = if content_adaptations.is_empty() {
        vec![create_fallback_video_adaptation_set(
            break_idx,
            segment_urls,
        )]
    } else {
        content_adaptations
            .iter()
            .enumerate()
            .map(|(as_idx, content_as)| {
                let bw = content_as
                    .representations
                    .first()
                    .and_then(|r| r.bandwidth)
                    .unwrap_or(500_000);

                let representation = Representation {
                    id: Some(format!("ad-rep-{}-{}", break_idx, as_idx)),
                    bandwidth: Some(bw),
                    SegmentList: Some(SegmentList {
                        segment_urls: segment_urls.clone(),
                        ..Default::default()
                    }),
                    ..Default::default()
                };

                AdaptationSet {
                    contentType: content_as.contentType.clone(),
                    mimeType: content_as.mimeType.clone(),
                    lang: content_as.lang.clone(),
                    representations: vec![representation],
                    ..Default::default()
                }
            })
            .collect()
    };

    // Build Period
    Period {
        id: Some(format!("ad-{}", break_idx)),
        duration: Some(Duration::from_secs_f64(total_duration)),
        adaptations,
        ..Default::default()
    }
}

/// Fallback: create a single video-only AdaptationSet (backward compatibility)
fn create_fallback_video_adaptation_set(
    break_idx: usize,
    segment_urls: Vec<SegmentURL>,
) -> AdaptationSet {
    let representation = Representation {
        id: Some(format!("ad-rep-{}", break_idx)),
        bandwidth: Some(500_000),
        SegmentList: Some(SegmentList {
            segment_urls,
            ..Default::default()
        }),
        ..Default::default()
    };

    AdaptationSet {
        contentType: Some("video".to_string()),
        mimeType: Some("video/mp4".to_string()),
        representations: vec![representation],
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dash::cue::{DashAdBreak, DashSignalType};

    fn create_test_mpd_with_periods(count: usize) -> MPD {
        let mut mpd = MPD::default();
        for i in 0..count {
            mpd.periods.push(Period {
                id: Some(format!("content-{}", i)),
                duration: Some(Duration::from_secs(60)),
                ..Default::default()
            });
        }
        mpd
    }

    /// Create a test MPD where each Period has video + audio AdaptationSets
    fn create_test_mpd_multi_track(count: usize) -> MPD {
        let mut mpd = MPD::default();
        for i in 0..count {
            mpd.periods.push(Period {
                id: Some(format!("content-{}", i)),
                duration: Some(Duration::from_secs(60)),
                adaptations: vec![
                    AdaptationSet {
                        contentType: Some("video".to_string()),
                        mimeType: Some("video/mp4".to_string()),
                        ..Default::default()
                    },
                    AdaptationSet {
                        contentType: Some("audio".to_string()),
                        mimeType: Some("audio/mp4".to_string()),
                        lang: Some("en".to_string()),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            });
        }
        mpd
    }

    fn create_test_ad_break(period_index: usize, duration: f64) -> DashAdBreak {
        DashAdBreak {
            period_index,
            period_id: Some(format!("content-{}", period_index)),
            duration,
            presentation_time: 0.0,
            signal_type: DashSignalType::SpliceInsert,
        }
    }

    #[test]
    fn test_interleave_single_ad_break() {
        let mpd = create_test_mpd_with_periods(2);

        let ad_breaks = vec![create_test_ad_break(0, 30.0)];

        let ad_segments = vec![vec![
            AdSegment {
                uri: "ad1.ts".to_string(),
                duration: 10.0,
                tracking: None,
            },
            AdSegment {
                uri: "ad2.ts".to_string(),
                duration: 10.0,
                tracking: None,
            },
            AdSegment {
                uri: "ad3.ts".to_string(),
                duration: 10.0,
                tracking: None,
            },
        ]];

        let result = interleave_ads_mpd(
            mpd,
            &ad_breaks,
            &ad_segments,
            "test-session",
            "http://stitcher",
        );

        // Should have 3 periods: content-0, ad-0, content-1
        assert_eq!(result.periods.len(), 3);
        assert_eq!(result.periods[0].id, Some("content-0".to_string()));
        assert_eq!(result.periods[1].id, Some("ad-0".to_string()));
        assert_eq!(result.periods[2].id, Some("content-1".to_string()));

        // Verify ad period has SegmentList with 3 segments
        // (content-0 has no AdaptationSets, so fallback to single video)
        let ad_period = &result.periods[1];
        assert_eq!(ad_period.adaptations.len(), 1);
        let adaptation_set = &ad_period.adaptations[0];
        assert_eq!(adaptation_set.representations.len(), 1);
        let representation = &adaptation_set.representations[0];
        assert!(representation.SegmentList.is_some());
        let segment_list = representation.SegmentList.as_ref().unwrap();
        assert_eq!(segment_list.segment_urls.len(), 3);

        // Verify duration (30 seconds total)
        assert_eq!(ad_period.duration, Some(Duration::from_secs(30)));
    }

    #[test]
    fn test_interleave_multiple_ad_breaks() {
        let mpd = create_test_mpd_with_periods(4);

        let ad_breaks = vec![create_test_ad_break(0, 15.0), create_test_ad_break(2, 20.0)];

        let ad_segments = vec![
            vec![AdSegment {
                uri: "ad1.ts".to_string(),
                duration: 15.0,
                tracking: None,
            }],
            vec![
                AdSegment {
                    uri: "ad2.ts".to_string(),
                    duration: 10.0,
                    tracking: None,
                },
                AdSegment {
                    uri: "ad3.ts".to_string(),
                    duration: 10.0,
                    tracking: None,
                },
            ],
        ];

        let result = interleave_ads_mpd(mpd, &ad_breaks, &ad_segments, "test", "http://test");

        // Should have 6 periods: content-0, ad-0, content-1, content-2, ad-1, content-3
        assert_eq!(result.periods.len(), 6);
        assert_eq!(result.periods[0].id, Some("content-0".to_string()));
        assert_eq!(result.periods[1].id, Some("ad-0".to_string()));
        assert_eq!(result.periods[2].id, Some("content-1".to_string()));
        assert_eq!(result.periods[3].id, Some("content-2".to_string()));
        assert_eq!(result.periods[4].id, Some("ad-1".to_string()));
        assert_eq!(result.periods[5].id, Some("content-3".to_string()));
    }

    #[test]
    fn test_interleave_no_ad_breaks() {
        let mpd = create_test_mpd_with_periods(2);
        let ad_breaks = vec![];
        let ad_segments = vec![];

        let result =
            interleave_ads_mpd(mpd.clone(), &ad_breaks, &ad_segments, "test", "http://test");

        // Should return unchanged MPD
        assert_eq!(result.periods.len(), mpd.periods.len());
    }

    #[test]
    fn test_interleave_preserves_content_periods() {
        let mpd = create_test_mpd_with_periods(3);
        let original_periods = mpd.periods.clone();

        let ad_breaks = vec![create_test_ad_break(1, 30.0)];

        let ad_segments = vec![vec![AdSegment {
            uri: "ad.ts".to_string(),
            duration: 30.0,
            tracking: None,
        }]];

        let result = interleave_ads_mpd(mpd, &ad_breaks, &ad_segments, "test", "http://test");

        // Verify original content periods are preserved
        assert_eq!(result.periods[0].id, original_periods[0].id);
        assert_eq!(result.periods[1].id, original_periods[1].id);
        // Ad period inserted at index 2
        assert_eq!(result.periods[2].id, Some("ad-0".to_string()));
        assert_eq!(result.periods[3].id, original_periods[2].id);
    }

    #[test]
    fn test_ad_period_segment_urls() {
        let mpd = create_test_mpd_with_periods(1);

        let ad_breaks = vec![create_test_ad_break(0, 30.0)];

        let ad_segments = vec![vec![
            AdSegment {
                uri: "ad1.ts".to_string(),
                duration: 10.0,
                tracking: None,
            },
            AdSegment {
                uri: "ad2.ts".to_string(),
                duration: 10.0,
                tracking: None,
            },
        ]];

        let result = interleave_ads_mpd(
            mpd,
            &ad_breaks,
            &ad_segments,
            "session123",
            "https://stitcher.example.com",
        );

        // Verify segment URLs have correct format
        let ad_period = &result.periods[1];
        let segment_list = &ad_period.adaptations[0].representations[0]
            .SegmentList
            .as_ref()
            .unwrap();

        assert_eq!(segment_list.segment_urls.len(), 2);
        assert_eq!(
            segment_list.segment_urls[0].media,
            Some("https://stitcher.example.com/stitch/session123/ad/break-0-seg-0.ts".to_string())
        );
        assert_eq!(
            segment_list.segment_urls[1].media,
            Some("https://stitcher.example.com/stitch/session123/ad/break-0-seg-1.ts".to_string())
        );
    }

    #[test]
    fn test_interleave_empty_ad_segments() {
        let mpd = create_test_mpd_with_periods(2);

        let ad_breaks = vec![create_test_ad_break(0, 30.0)];

        let ad_segments = vec![vec![]]; // Empty ad segment list

        let result =
            interleave_ads_mpd(mpd.clone(), &ad_breaks, &ad_segments, "test", "http://test");

        // Should return unchanged MPD (empty ad segments skipped)
        assert_eq!(result.periods.len(), mpd.periods.len());
    }

    // --- Multi-track tests ---

    #[test]
    fn test_ad_period_mirrors_video_and_audio_adaptation_sets() {
        let mpd = create_test_mpd_multi_track(2);

        let ad_breaks = vec![create_test_ad_break(0, 30.0)];
        let ad_segments = vec![vec![AdSegment {
            uri: "ad.ts".to_string(),
            duration: 30.0,
            tracking: None,
        }]];

        let result = interleave_ads_mpd(mpd, &ad_breaks, &ad_segments, "test", "http://test");

        let ad_period = &result.periods[1];
        assert_eq!(ad_period.id, Some("ad-0".to_string()));

        // Ad Period should have 2 AdaptationSets mirroring content
        assert_eq!(ad_period.adaptations.len(), 2);

        // First: video
        assert_eq!(
            ad_period.adaptations[0].contentType,
            Some("video".to_string())
        );
        assert_eq!(
            ad_period.adaptations[0].mimeType,
            Some("video/mp4".to_string())
        );

        // Second: audio
        assert_eq!(
            ad_period.adaptations[1].contentType,
            Some("audio".to_string())
        );
        assert_eq!(
            ad_period.adaptations[1].mimeType,
            Some("audio/mp4".to_string())
        );
    }

    #[test]
    fn test_ad_period_preserves_lang_attribute() {
        let mpd = create_test_mpd_multi_track(1);

        let ad_breaks = vec![create_test_ad_break(0, 15.0)];
        let ad_segments = vec![vec![AdSegment {
            uri: "ad.ts".to_string(),
            duration: 15.0,
            tracking: None,
        }]];

        let result = interleave_ads_mpd(mpd, &ad_breaks, &ad_segments, "test", "http://test");

        let ad_period = &result.periods[1];
        let audio_as = &ad_period.adaptations[1];

        assert_eq!(audio_as.lang, Some("en".to_string()));
    }

    #[test]
    fn test_ad_period_fallback_when_no_content_adaptations() {
        // Periods without AdaptationSets → fallback to single video
        let mpd = create_test_mpd_with_periods(1);

        let ad_breaks = vec![create_test_ad_break(0, 10.0)];
        let ad_segments = vec![vec![AdSegment {
            uri: "ad.ts".to_string(),
            duration: 10.0,
            tracking: None,
        }]];

        let result = interleave_ads_mpd(mpd, &ad_breaks, &ad_segments, "test", "http://test");

        let ad_period = &result.periods[1];
        assert_eq!(ad_period.adaptations.len(), 1);
        assert_eq!(
            ad_period.adaptations[0].contentType,
            Some("video".to_string())
        );
    }

    #[test]
    fn test_ad_period_shared_segment_urls_across_tracks() {
        let mpd = create_test_mpd_multi_track(1);

        let ad_breaks = vec![create_test_ad_break(0, 20.0)];
        let ad_segments = vec![vec![
            AdSegment {
                uri: "ad1.ts".to_string(),
                duration: 10.0,
                tracking: None,
            },
            AdSegment {
                uri: "ad2.ts".to_string(),
                duration: 10.0,
                tracking: None,
            },
        ]];

        let result = interleave_ads_mpd(mpd, &ad_breaks, &ad_segments, "test", "http://test");

        let ad_period = &result.periods[1];

        // Both AdaptationSets should have identical SegmentList URLs
        let video_urls: Vec<_> = ad_period.adaptations[0].representations[0]
            .SegmentList
            .as_ref()
            .unwrap()
            .segment_urls
            .iter()
            .map(|u| u.media.as_ref().unwrap())
            .collect();

        let audio_urls: Vec<_> = ad_period.adaptations[1].representations[0]
            .SegmentList
            .as_ref()
            .unwrap()
            .segment_urls
            .iter()
            .map(|u| u.media.as_ref().unwrap())
            .collect();

        assert_eq!(video_urls, audio_urls);
        assert_eq!(video_urls.len(), 2);
    }
}
