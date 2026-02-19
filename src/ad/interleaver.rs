use crate::ad::provider::AdSegment;
use crate::hls::cue::AdBreak;
use m3u8_rs::{MediaPlaylist, MediaSegment};
use tracing::{info, warn};

/// Interleave ad segments into a playlist based on detected ad breaks
///
/// Replaces content segments within ad break windows with ad segments,
/// adding proper `#EXT-X-DISCONTINUITY` tags before and after each ad break.
///
/// # Arguments
/// * `playlist` - The parsed MediaPlaylist to modify
/// * `ad_breaks` - Detected ad break positions from CUE tags
/// * `ad_segments` - Ad segments to insert (one vec per ad break)
/// * `session_id` - Session ID for URL generation
/// * `base_url` - Base URL for the stitcher
///
/// # Returns
/// Modified MediaPlaylist with ad segments interleaved
pub fn interleave_ads(
    mut playlist: MediaPlaylist,
    ad_breaks: &[AdBreak],
    ad_segments_per_break: &[Vec<AdSegment>],
    session_id: &str,
    base_url: &str,
) -> MediaPlaylist {
    if ad_breaks.is_empty() {
        info!("No ad breaks detected, returning playlist unchanged");
        return playlist;
    }

    if ad_breaks.len() != ad_segments_per_break.len() {
        warn!(
            "Mismatch between ad breaks ({}) and ad segment sets ({})",
            ad_breaks.len(),
            ad_segments_per_break.len()
        );
        return playlist;
    }

    let mut new_segments = Vec::new();
    let mut segment_index = 0;
    let original_segments = std::mem::take(&mut playlist.segments);

    for (break_idx, ad_break) in ad_breaks.iter().enumerate() {
        // Add content segments before this ad break
        while segment_index < ad_break.start_index && segment_index < original_segments.len() {
            new_segments.push(original_segments[segment_index].clone());
            segment_index += 1;
        }

        // Insert ad segments with discontinuity markers
        let ad_segments = &ad_segments_per_break[break_idx];
        if !ad_segments.is_empty() {
            info!(
                "Inserting {} ad segments at position {} (ad break {}/{})",
                ad_segments.len(),
                segment_index,
                break_idx + 1,
                ad_breaks.len()
            );

            // Add discontinuity before first ad segment
            let mut first_ad_segment =
                create_media_segment_from_ad(&ad_segments[0], session_id, base_url, break_idx, 0);
            first_ad_segment.discontinuity = true;
            new_segments.push(first_ad_segment);

            // Add remaining ad segments
            for (idx, ad_segment) in ad_segments.iter().skip(1).enumerate() {
                let media_segment = create_media_segment_from_ad(
                    ad_segment,
                    session_id,
                    base_url,
                    break_idx,
                    idx + 1,
                );
                new_segments.push(media_segment);
            }

            // Skip the original content segments that were in the ad break window
            segment_index = ad_break.end_index;

            // Add discontinuity after last ad segment (if there are more content segments)
            if segment_index < original_segments.len() {
                // Set discontinuity on the next content segment
                if let Some(next_segment) = original_segments.get(segment_index) {
                    let mut next = next_segment.clone();
                    next.discontinuity = true;
                    new_segments.push(next);
                    segment_index += 1;
                }
            }
        }
    }

    // Add any remaining content segments after the last ad break
    while segment_index < original_segments.len() {
        new_segments.push(original_segments[segment_index].clone());
        segment_index += 1;
    }

    info!(
        "Interleaving complete: {} original segments → {} segments with ads",
        original_segments.len(),
        new_segments.len()
    );

    playlist.segments = new_segments;
    playlist
}

/// Create a MediaSegment from an AdSegment
fn create_media_segment_from_ad(
    ad_segment: &AdSegment,
    session_id: &str,
    base_url: &str,
    break_idx: usize,
    segment_idx: usize,
) -> MediaSegment {
    // Route ad segment through the stitcher's ad handler
    // Format: /stitch/{session_id}/ad/break-{break_idx}-seg-{segment_idx}.ts
    let stitcher_uri = format!(
        "{}/stitch/{}/ad/break-{}-seg-{}.ts",
        base_url, session_id, break_idx, segment_idx
    );

    MediaSegment {
        uri: stitcher_uri,
        duration: ad_segment.duration,
        title: Some(format!("Ad Break {}", break_idx + 1)),
        byte_range: None,
        discontinuity: false, // Set by caller when needed
        key: None,
        map: None,
        program_date_time: None,
        daterange: None,
        unknown_tags: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_segment(uri: &str, duration: f32) -> MediaSegment {
        MediaSegment {
            uri: uri.to_string(),
            duration,
            title: None,
            byte_range: None,
            discontinuity: false,
            key: None,
            map: None,
            program_date_time: None,
            daterange: None,
            unknown_tags: Vec::new(),
        }
    }

    #[test]
    fn test_interleave_single_ad_break() {
        let playlist = MediaPlaylist {
            segments: vec![
                create_test_segment("seg0.ts", 10.0),
                create_test_segment("seg1.ts", 10.0),
                create_test_segment("seg2.ts", 10.0),
                create_test_segment("seg3.ts", 10.0),
                create_test_segment("seg4.ts", 10.0),
            ],
            ..Default::default()
        };

        let ad_breaks = vec![AdBreak {
            start_index: 1,
            end_index: 3,
            duration: 30.0,
        }];

        let ad_segments = vec![vec![
            AdSegment {
                uri: "ad1.ts".to_string(),
                duration: 15.0,
                tracking: None,
            },
            AdSegment {
                uri: "ad2.ts".to_string(),
                duration: 15.0,
                tracking: None,
            },
        ]];

        let result = interleave_ads(
            playlist,
            &ad_breaks,
            &ad_segments,
            "test-session",
            "http://localhost",
        );

        // Should have: seg0, ad1, ad2, seg3(with discontinuity), seg4
        assert_eq!(result.segments.len(), 5);
        assert_eq!(result.segments[0].uri, "seg0.ts");
        assert!(result.segments[1].uri.contains("/ad/break-0-seg-0.ts"));
        assert!(result.segments[1].discontinuity); // Discontinuity before first ad
        assert!(result.segments[2].uri.contains("/ad/break-0-seg-1.ts"));
        assert!(!result.segments[2].discontinuity);
        assert_eq!(result.segments[3].uri, "seg3.ts");
        assert!(result.segments[3].discontinuity); // Discontinuity after last ad
        assert_eq!(result.segments[4].uri, "seg4.ts");
    }

    #[test]
    fn test_interleave_multiple_ad_breaks() {
        let playlist = MediaPlaylist {
            segments: vec![
                create_test_segment("seg0.ts", 10.0),
                create_test_segment("seg1.ts", 10.0),
                create_test_segment("seg2.ts", 10.0),
                create_test_segment("seg3.ts", 10.0),
                create_test_segment("seg4.ts", 10.0),
                create_test_segment("seg5.ts", 10.0),
            ],
            ..Default::default()
        };

        let ad_breaks = vec![
            AdBreak {
                start_index: 1,
                end_index: 2,
                duration: 15.0,
            },
            AdBreak {
                start_index: 4,
                end_index: 5,
                duration: 15.0,
            },
        ];

        let ad_segments = vec![
            vec![AdSegment {
                uri: "ad1.ts".to_string(),
                duration: 15.0,
                tracking: None,
            }],
            vec![AdSegment {
                uri: "ad2.ts".to_string(),
                duration: 15.0,
                tracking: None,
            }],
        ];

        let result = interleave_ads(
            playlist,
            &ad_breaks,
            &ad_segments,
            "test-session",
            "http://localhost",
        );

        // seg0, ad1, seg2, seg3, ad2, seg5
        assert_eq!(result.segments.len(), 6);
        assert_eq!(result.segments[0].uri, "seg0.ts");
        assert!(result.segments[1].discontinuity); // Before first ad
        assert_eq!(result.segments[2].uri, "seg2.ts");
        assert!(result.segments[2].discontinuity); // After first ad
        assert_eq!(result.segments[3].uri, "seg3.ts");
        assert!(result.segments[4].discontinuity); // Before second ad
        assert_eq!(result.segments[5].uri, "seg5.ts");
        assert!(result.segments[5].discontinuity); // After second ad
    }

    #[test]
    fn test_interleave_no_ad_breaks() {
        let playlist = MediaPlaylist {
            segments: vec![
                create_test_segment("seg0.ts", 10.0),
                create_test_segment("seg1.ts", 10.0),
            ],
            ..Default::default()
        };

        let result = interleave_ads(
            playlist.clone(),
            &[],
            &[],
            "test-session",
            "http://localhost",
        );

        assert_eq!(result.segments.len(), 2);
        assert_eq!(result.segments[0].uri, "seg0.ts");
        assert_eq!(result.segments[1].uri, "seg1.ts");
    }
}
