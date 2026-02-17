use m3u8_rs::MediaPlaylist;
use tracing::{debug, info};

/// Represents an ad break detected from CUE tags in the playlist
#[derive(Debug, Clone, PartialEq)]
pub struct AdBreak {
    /// Starting segment index (inclusive)
    pub start_index: usize,
    /// Ending segment index (exclusive)
    pub end_index: usize,
    /// Duration of the ad break in seconds
    pub duration: f32,
}

/// Detect ad breaks from SCTE-35 CUE tags in HLS playlists
///
/// Scans the `unknown_tags` field of each MediaSegment for industry-standard
/// CUE markers:
/// - `#EXT-X-CUE-OUT:{duration}` — ad break start
/// - `#EXT-X-CUE-OUT-CONT:{elapsed}/{duration}` — mid-break continuation
/// - `#EXT-X-CUE-IN` — ad break end
///
/// Note: m3u8-rs strips the `#EXT-` prefix from unknown tags, so the tag
/// field contains e.g. `X-CUE-OUT` (not `EXT-X-CUE-OUT`).
///
/// Returns a vector of AdBreak structs with start/end indices and duration.
pub fn detect_ad_breaks(playlist: &MediaPlaylist) -> Vec<AdBreak> {
    let mut ad_breaks = Vec::new();
    let mut current_break: Option<(usize, f32)> = None; // (start_index, duration)

    for (index, segment) in playlist.segments.iter().enumerate() {
        // Check unknown_tags for CUE markers
        for tag in &segment.unknown_tags {
            // Match against tag name directly (m3u8-rs strips #EXT- prefix)
            // CUE-IN: tag.tag is "X-CUE-IN", rest is None
            if is_cue_in(&tag.tag) {
                if let Some((start_idx, duration)) = current_break.take() {
                    info!("Detected CUE-IN at segment #{}", index);
                    ad_breaks.push(AdBreak {
                        start_index: start_idx,
                        end_index: index,
                        duration,
                    });
                }
            }
            // CUE-OUT-CONT: tag.tag is "X-CUE-OUT-CONT", rest is e.g. "10/30"
            else if is_cue_out_cont(&tag.tag) {
                debug!("Detected CUE-OUT-CONT at segment #{}", index);
            }
            // CUE-OUT: tag.tag is "X-CUE-OUT", rest is e.g. "30" or "DURATION=30"
            else if let Some(duration) = parse_cue_out(&tag.tag, tag.rest.as_deref()) {
                info!(
                    "Detected CUE-OUT at segment #{}: duration {}s",
                    index, duration
                );
                if current_break.is_none() {
                    current_break = Some((index, duration));
                }
            }
        }
    }

    // If we reached the end with an open ad break, close it
    if let Some((start_idx, duration)) = current_break {
        info!(
            "Ad break started at segment #{} not closed, ending at playlist end",
            start_idx
        );
        ad_breaks.push(AdBreak {
            start_index: start_idx,
            end_index: playlist.segments.len(),
            duration,
        });
    }

    ad_breaks
}

/// Check if a tag name represents CUE-IN
///
/// m3u8-rs strips `#EXT-` so we check for `X-CUE-IN` and `CUE-IN`
fn is_cue_in(tag_name: &str) -> bool {
    tag_name == "X-CUE-IN" || tag_name == "CUE-IN"
}

/// Check if a tag name represents CUE-OUT-CONT
fn is_cue_out_cont(tag_name: &str) -> bool {
    tag_name == "X-CUE-OUT-CONT" || tag_name == "CUE-OUT-CONT"
}

/// Parse CUE-OUT tag to extract duration
///
/// m3u8-rs splits unknown tags into `tag` (the name) and `rest` (after the colon).
///
/// Supports formats:
/// - tag="X-CUE-OUT", rest=Some("30") → 30.0
/// - tag="X-CUE-OUT", rest=Some("DURATION=30") → 30.0
/// - tag="CUE-OUT", rest=Some("30") → 30.0 (legacy format)
fn parse_cue_out(tag_name: &str, rest: Option<&str>) -> Option<f32> {
    // Must be CUE-OUT but not CUE-OUT-CONT
    if !(tag_name == "X-CUE-OUT" || tag_name == "CUE-OUT") {
        return None;
    }

    let rest = rest?;

    // Handle "DURATION=30" format
    if let Some(eq_pos) = rest.find('=') {
        let duration_str = &rest[eq_pos + 1..];
        if let Ok(duration) = duration_str.trim().parse::<f32>() {
            return Some(duration);
        }
    }

    // Handle simple "30" format
    if let Ok(duration) = rest.trim().parse::<f32>() {
        return Some(duration);
    }

    None
}

/// Helper to check if a segment is within an ad break
pub fn is_in_ad_break(segment_index: usize, ad_breaks: &[AdBreak]) -> bool {
    ad_breaks
        .iter()
        .any(|ab| segment_index >= ab.start_index && segment_index < ab.end_index)
}

#[cfg(test)]
mod tests {
    use super::*;
    use m3u8_rs::{ExtTag, MediaSegment};

    fn create_segment(uri: &str) -> MediaSegment {
        MediaSegment {
            uri: uri.to_string(),
            duration: 10.0,
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

    fn create_segment_with_tag(tag: &str, rest: Option<&str>) -> MediaSegment {
        MediaSegment {
            uri: "segment.ts".to_string(),
            duration: 10.0,
            title: None,
            byte_range: None,
            discontinuity: false,
            key: None,
            map: None,
            program_date_time: None,
            daterange: None,
            unknown_tags: vec![ExtTag {
                tag: tag.to_string(),
                rest: rest.map(|s| s.to_string()),
            }],
        }
    }

    #[test]
    fn test_parse_cue_out_simple() {
        assert_eq!(parse_cue_out("X-CUE-OUT", Some("30")), Some(30.0));
        assert_eq!(parse_cue_out("X-CUE-OUT", Some("60.5")), Some(60.5));
    }

    #[test]
    fn test_parse_cue_out_with_duration_key() {
        assert_eq!(
            parse_cue_out("X-CUE-OUT", Some("DURATION=30")),
            Some(30.0)
        );
        assert_eq!(
            parse_cue_out("X-CUE-OUT", Some("DURATION=45.5")),
            Some(45.5)
        );
    }

    #[test]
    fn test_parse_cue_out_legacy() {
        assert_eq!(parse_cue_out("CUE-OUT", Some("30")), Some(30.0));
    }

    #[test]
    fn test_parse_cue_out_invalid() {
        assert_eq!(parse_cue_out("X-CUE-OUT-CONT", Some("10/30")), None);
        assert_eq!(parse_cue_out("X-CUE-IN", None), None);
        assert_eq!(parse_cue_out("X-CUE-OUT", Some("invalid")), None);
        assert_eq!(parse_cue_out("X-CUE-OUT", None), None);
    }

    #[test]
    fn test_is_cue_in() {
        assert!(is_cue_in("X-CUE-IN"));
        assert!(is_cue_in("CUE-IN"));
        assert!(!is_cue_in("X-CUE-OUT"));
        assert!(!is_cue_in("SOMETHING"));
    }

    #[test]
    fn test_detect_ad_breaks_simple() {
        // Use tag names as m3u8-rs stores them (without #EXT- prefix)
        let playlist = MediaPlaylist {
            segments: vec![
                create_segment("seg0.ts"),
                create_segment_with_tag("X-CUE-OUT", Some("30")),
                create_segment("seg2.ts"),
                create_segment("seg3.ts"),
                create_segment_with_tag("X-CUE-IN", None),
                create_segment("seg5.ts"),
            ],
            ..Default::default()
        };

        let ad_breaks = detect_ad_breaks(&playlist);

        assert_eq!(ad_breaks.len(), 1);
        assert_eq!(
            ad_breaks[0],
            AdBreak {
                start_index: 1,
                end_index: 4,
                duration: 30.0
            }
        );
    }

    #[test]
    fn test_detect_multiple_ad_breaks() {
        let playlist = MediaPlaylist {
            segments: vec![
                create_segment("seg0.ts"),
                create_segment_with_tag("X-CUE-OUT", Some("30")),
                create_segment("seg2.ts"),
                create_segment_with_tag("X-CUE-IN", None),
                create_segment("seg4.ts"),
                create_segment_with_tag("X-CUE-OUT", Some("60")),
                create_segment("seg6.ts"),
                create_segment_with_tag("X-CUE-IN", None),
            ],
            ..Default::default()
        };

        let ad_breaks = detect_ad_breaks(&playlist);

        assert_eq!(ad_breaks.len(), 2);
        assert_eq!(ad_breaks[0].start_index, 1);
        assert_eq!(ad_breaks[0].end_index, 3);
        assert_eq!(ad_breaks[0].duration, 30.0);
        assert_eq!(ad_breaks[1].start_index, 5);
        assert_eq!(ad_breaks[1].end_index, 7);
        assert_eq!(ad_breaks[1].duration, 60.0);
    }

    #[test]
    fn test_detect_unclosed_ad_break() {
        let playlist = MediaPlaylist {
            segments: vec![
                create_segment("seg0.ts"),
                create_segment_with_tag("X-CUE-OUT", Some("30")),
                create_segment("seg2.ts"),
            ],
            ..Default::default()
        };

        let ad_breaks = detect_ad_breaks(&playlist);

        assert_eq!(ad_breaks.len(), 1);
        assert_eq!(ad_breaks[0].start_index, 1);
        assert_eq!(ad_breaks[0].end_index, 3);
    }

    #[test]
    fn test_detect_with_cue_out_cont() {
        // Simulate what m3u8-rs actually produces from a real playlist
        let playlist = MediaPlaylist {
            segments: vec![
                create_segment("seg0.ts"),
                create_segment_with_tag("X-CUE-OUT", Some("30")),
                create_segment_with_tag("X-CUE-OUT-CONT", Some("10/30")),
                create_segment_with_tag("X-CUE-OUT-CONT", Some("20/30")),
                create_segment_with_tag("X-CUE-IN", None),
                create_segment("seg5.ts"),
            ],
            ..Default::default()
        };

        let ad_breaks = detect_ad_breaks(&playlist);

        assert_eq!(ad_breaks.len(), 1);
        assert_eq!(ad_breaks[0].start_index, 1);
        assert_eq!(ad_breaks[0].end_index, 4);
        assert_eq!(ad_breaks[0].duration, 30.0);
    }

    #[test]
    fn test_is_in_ad_break() {
        let ad_breaks = vec![AdBreak {
            start_index: 2,
            end_index: 5,
            duration: 30.0,
        }];

        assert!(!is_in_ad_break(0, &ad_breaks));
        assert!(!is_in_ad_break(1, &ad_breaks));
        assert!(is_in_ad_break(2, &ad_breaks));
        assert!(is_in_ad_break(3, &ad_breaks));
        assert!(is_in_ad_break(4, &ad_breaks));
        assert!(!is_in_ad_break(5, &ad_breaks));
    }
}
