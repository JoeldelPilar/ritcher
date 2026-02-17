use crate::ad::provider::AdSegment;
use crate::dash::cue::DashAdBreak;
use dash_mpd::MPD;
use tracing::info;

/// Interleave ad segments into DASH MPD by replacing ad periods
///
/// This is a STUB implementation for Phase 2 Week 1-2.
/// Full Period-based ad insertion will be implemented in Week 3.
///
/// For now, this function returns the MPD unchanged to allow the rest
/// of the codebase to compile and tests to pass.
///
/// # Arguments
/// * `mpd` - The original MPD to modify
/// * `ad_breaks` - Detected ad breaks from EventStream/SCTE-35
/// * `ad_segments` - Ad segments to insert (one Vec per ad break)
/// * `session_id` - Session ID for URL generation
/// * `base_url` - Stitcher base URL for proxying
///
/// # Returns
/// Modified MPD with ad Periods inserted (currently just returns input MPD)
pub fn interleave_ads_mpd(
    mpd: MPD,
    _ad_breaks: &[DashAdBreak],
    _ad_segments: &[Vec<AdSegment>],
    _session_id: &str,
    _base_url: &str,
) -> MPD {
    info!(
        "interleave_ads_mpd called (stub implementation - no modifications made)"
    );

    // TODO (Week 3): Implement Period-based ad insertion
    // 1. For each DashAdBreak:
    //    - Create a new Period with ad AdaptationSet + Representation
    //    - Use SegmentList with explicit segment URLs pointing to stitcher ad proxy
    //    - Set Period duration to match ad content
    // 2. Insert ad Periods at correct positions in MPD
    // 3. Maintain timing continuity between Periods

    mpd // Return unmodified MPD for now
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dash::cue::{DashAdBreak, DashSignalType};

    #[test]
    fn test_interleave_ads_mpd_stub() {
        // Create a minimal MPD
        let mpd = MPD::default();

        // Create stub ad break
        let ad_breaks = vec![DashAdBreak {
            period_index: 0,
            period_id: Some("test".to_string()),
            duration: 30.0,
            presentation_time: 0.0,
            signal_type: DashSignalType::SpliceInsert,
        }];

        // Create stub ad segments
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

        // Call stub interleaver
        let result = interleave_ads_mpd(mpd.clone(), &ad_breaks, &ad_segments, "test-session", "http://stitcher");

        // For now, just verify it returns the same MPD (stub behavior)
        assert_eq!(result.periods.len(), mpd.periods.len());
    }

    #[test]
    fn test_interleave_no_ad_breaks() {
        let mpd = MPD::default();
        let ad_breaks = vec![];
        let ad_segments = vec![];

        let result = interleave_ads_mpd(mpd.clone(), &ad_breaks, &ad_segments, "test", "http://test");

        // Should return unchanged MPD
        assert_eq!(result.periods.len(), mpd.periods.len());
    }
}
