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

        info!(
            "Inserting {} ad segments at Period {} (ad break {}/{})",
            ad_segments.len(),
            ad_break.period_index,
            break_idx + 1,
            ad_breaks.len()
        );

        // Create ad Period
        let ad_period = create_ad_period(ad_segments, break_idx, session_id, base_url);

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
/// Builds: Period → AdaptationSet → Representation → SegmentList → Vec<SegmentURL>
///
/// # Arguments
/// * `ad_segments` - Ad segments to include in this Period
/// * `break_idx` - Index of this ad break (for ID generation)
/// * `session_id` - Session ID for URL generation
/// * `base_url` - Stitcher base URL for proxying
///
/// # Returns
/// A Period with ad content
fn create_ad_period(
    ad_segments: &[AdSegment],
    break_idx: usize,
    session_id: &str,
    base_url: &str,
) -> Period {
    // Calculate total duration
    let total_duration: f64 = ad_segments.iter().map(|s| s.duration as f64).sum();

    // Create SegmentURL entries for each ad segment
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

    // Build SegmentList
    let segment_list = SegmentList {
        segment_urls,
        ..Default::default()
    };

    // Build Representation
    let representation = Representation {
        id: Some(format!("ad-rep-{}", break_idx)),
        bandwidth: Some(500_000), // Conservative bandwidth estimate for ads
        SegmentList: Some(segment_list),
        ..Default::default()
    };

    // Build AdaptationSet
    let adaptation_set = AdaptationSet {
        contentType: Some("video".to_string()),
        mimeType: Some("video/mp4".to_string()),
        representations: vec![representation],
        ..Default::default()
    };

    // Build Period
    Period {
        id: Some(format!("ad-{}", break_idx)),
        duration: Some(Duration::from_secs_f64(total_duration)),
        adaptations: vec![adaptation_set],
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

    #[test]
    fn test_interleave_single_ad_break() {
        let mpd = create_test_mpd_with_periods(2);

        let ad_breaks = vec![DashAdBreak {
            period_index: 0,
            period_id: Some("content-0".to_string()),
            duration: 30.0,
            presentation_time: 0.0,
            signal_type: DashSignalType::SpliceInsert,
        }];

        let ad_segments = vec![vec![
            AdSegment {
                uri: "ad1.ts".to_string(),
                duration: 10.0,
            },
            AdSegment {
                uri: "ad2.ts".to_string(),
                duration: 10.0,
            },
            AdSegment {
                uri: "ad3.ts".to_string(),
                duration: 10.0,
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

        let ad_breaks = vec![
            DashAdBreak {
                period_index: 0,
                period_id: Some("content-0".to_string()),
                duration: 15.0,
                presentation_time: 0.0,
                signal_type: DashSignalType::SpliceInsert,
            },
            DashAdBreak {
                period_index: 2,
                period_id: Some("content-2".to_string()),
                duration: 20.0,
                presentation_time: 0.0,
                signal_type: DashSignalType::SpliceInsert,
            },
        ];

        let ad_segments = vec![
            vec![AdSegment {
                uri: "ad1.ts".to_string(),
                duration: 15.0,
            }],
            vec![
                AdSegment {
                    uri: "ad2.ts".to_string(),
                    duration: 10.0,
                },
                AdSegment {
                    uri: "ad3.ts".to_string(),
                    duration: 10.0,
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

        let ad_breaks = vec![DashAdBreak {
            period_index: 1,
            period_id: Some("content-1".to_string()),
            duration: 30.0,
            presentation_time: 0.0,
            signal_type: DashSignalType::SpliceInsert,
        }];

        let ad_segments = vec![vec![AdSegment {
            uri: "ad.ts".to_string(),
            duration: 30.0,
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

        let ad_breaks = vec![DashAdBreak {
            period_index: 0,
            period_id: Some("content-0".to_string()),
            duration: 30.0,
            presentation_time: 0.0,
            signal_type: DashSignalType::SpliceInsert,
        }];

        let ad_segments = vec![vec![
            AdSegment {
                uri: "ad1.ts".to_string(),
                duration: 10.0,
            },
            AdSegment {
                uri: "ad2.ts".to_string(),
                duration: 10.0,
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

        let ad_breaks = vec![DashAdBreak {
            period_index: 0,
            period_id: Some("content-0".to_string()),
            duration: 30.0,
            presentation_time: 0.0,
            signal_type: DashSignalType::SpliceInsert,
        }];

        let ad_segments = vec![vec![]]; // Empty ad segment list

        let result =
            interleave_ads_mpd(mpd.clone(), &ad_breaks, &ad_segments, "test", "http://test");

        // Should return unchanged MPD (empty ad segments skipped)
        assert_eq!(result.periods.len(), mpd.periods.len());
    }
}
