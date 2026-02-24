//! HLS Interstitials support for Server-Guided Ad Insertion (SGAI)
//!
//! Implements the Apple HLS Interstitials specification (RFC 8216bis):
//! - EXT-X-PROGRAM-DATE-TIME synthesis when origin playlist lacks timing info
//! - EXT-X-DATERANGE injection with CLASS="com.apple.hls.interstitial"
//!
//! In SGAI mode the stitcher does NOT replace content segments. Instead it
//! signals ad break opportunities via DateRange tags. The player (hls.js ≥1.6,
//! AVPlayer) fetches ad content directly from the ad CDN via the X-ASSET-LIST
//! URL and handles playback client-side.

use crate::hls::cue::AdBreak;
use chrono::{DateTime, FixedOffset, TimeZone};
use m3u8_rs::{DateRange, MediaPlaylist, QuotedOrUnquoted};
use std::collections::HashMap;
use tracing::info;

/// Synthetic base time used when origin playlist has no EXT-X-PROGRAM-DATE-TIME.
///
/// RFC 3339 fixed offset zero — 2026-01-01T00:00:00+00:00
fn synthetic_base_time() -> DateTime<FixedOffset> {
    FixedOffset::east_opt(0)
        .expect("UTC offset is valid")
        .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
        .single()
        .expect("2026-01-01 00:00:00 is a valid datetime")
}

/// Ensure every segment has a program_date_time value.
///
/// If the playlist already carries PDT on any segment the function is a no-op
/// (existing timing is preserved). Otherwise a synthetic PDT is assigned to
/// every segment starting from a fixed epoch, accumulating segment durations.
///
/// PDT is required by the HLS Interstitials spec: DateRange START-DATE values
/// are interpreted relative to the PDT timeline.
pub fn ensure_program_date_time(playlist: &mut MediaPlaylist) {
    let has_pdt = playlist
        .segments
        .iter()
        .any(|s| s.program_date_time.is_some());

    if has_pdt {
        return;
    }

    info!("SGAI: No EXT-X-PROGRAM-DATE-TIME found — synthesizing from epoch");

    let base = synthetic_base_time();
    let mut offset_ms: i64 = 0;

    for seg in playlist.segments.iter_mut() {
        let pdt = base + chrono::Duration::milliseconds(offset_ms);
        seg.program_date_time = Some(pdt);
        offset_ms += (seg.duration * 1000.0) as i64;
    }
}

/// Inject EXT-X-DATERANGE interstitial markers for each ad break.
///
/// For every detected `AdBreak`:
/// 1. Computes the START-DATE from the segment's program_date_time at `start_index`
/// 2. Builds a DateRange with `CLASS="com.apple.hls.interstitial"` and the
///    standard HLS Interstitials attributes
/// 3. Sets the DateRange on the segment at `start_index`
/// 4. Strips the SCTE-35 CUE-OUT/CUE-IN/CUE-OUT-CONT tags from unknown_tags
///    (they would confuse players that also parse DateRange interstitials)
///
/// Call `ensure_program_date_time` before this function.
pub fn inject_interstitials(
    playlist: &mut MediaPlaylist,
    ad_breaks: &[AdBreak],
    session_id: &str,
    base_url: &str,
) {
    for (break_idx, ad_break) in ad_breaks.iter().enumerate() {
        let start_index = ad_break.start_index;

        // Guard: break must reference a valid segment
        if start_index >= playlist.segments.len() {
            continue;
        }

        let start_date = match compute_pdt_at(playlist, start_index) {
            Some(dt) => dt,
            None => {
                // Should not happen after ensure_program_date_time(), but be safe
                info!(
                    "SGAI: No PDT available for segment {} — skipping interstitial injection",
                    start_index
                );
                continue;
            }
        };

        let asset_list_url = format!(
            "{}/stitch/{}/asset-list/{}?dur={}",
            base_url, session_id, break_idx, ad_break.duration
        );

        info!(
            "SGAI: Injecting interstitial at segment #{}: duration={}s asset-list={}",
            start_index, ad_break.duration, asset_list_url
        );

        let mut x_prefixed = HashMap::new();
        x_prefixed.insert(
            "X-ASSET-LIST".to_string(),
            QuotedOrUnquoted::Quoted(asset_list_url),
        );
        // X-RESUME-OFFSET=0 — resume content at the break point (no gap)
        x_prefixed.insert(
            "X-RESUME-OFFSET".to_string(),
            QuotedOrUnquoted::Unquoted("0".to_string()),
        );
        // X-RESTRICT — prevent the player from allowing skip/seek past the ad
        x_prefixed.insert(
            "X-RESTRICT".to_string(),
            QuotedOrUnquoted::Quoted("SKIP,JUMP".to_string()),
        );

        let daterange = DateRange {
            id: format!("ad-break-{}", break_idx),
            class: Some("com.apple.hls.interstitial".to_string()),
            start_date,
            end_date: None,
            duration: Some(ad_break.duration as f64),
            planned_duration: None,
            x_prefixed: Some(x_prefixed),
            end_on_next: false,
            other_attributes: None,
        };

        playlist.segments[start_index].daterange = Some(daterange);
    }

    // Strip CUE-OUT/CUE-IN/CUE-OUT-CONT tags — they conflict with DateRange interstitials
    remove_cue_tags(playlist);
}

/// Remove SCTE-35 CUE tags from all segment unknown_tags.
fn remove_cue_tags(playlist: &mut MediaPlaylist) {
    for seg in playlist.segments.iter_mut() {
        seg.unknown_tags.retain(|tag| !is_cue_tag(&tag.tag));
    }
}

/// Returns true for CUE-OUT, CUE-OUT-CONT, and CUE-IN tag names
/// (both with and without the X- prefix that m3u8-rs strips).
fn is_cue_tag(tag_name: &str) -> bool {
    matches!(
        tag_name,
        "X-CUE-OUT" | "CUE-OUT" | "X-CUE-OUT-CONT" | "CUE-OUT-CONT" | "X-CUE-IN" | "CUE-IN"
    )
}

/// Compute the program_date_time for the segment at `target_index` by
/// walking forward from the nearest preceding segment that has PDT set.
///
/// Returns None only if no segment at or before `target_index` has PDT.
fn compute_pdt_at(playlist: &MediaPlaylist, target_index: usize) -> Option<DateTime<FixedOffset>> {
    // Find the last segment ≤ target_index that has an explicit PDT anchor
    let (anchor_index, anchor_pdt) = playlist
        .segments
        .iter()
        .enumerate()
        .take(target_index + 1)
        .filter_map(|(i, seg)| seg.program_date_time.map(|pdt| (i, pdt)))
        .next_back()?;

    // Accumulate duration from anchor to target
    let offset_ms: i64 = playlist.segments[anchor_index..target_index]
        .iter()
        .map(|s| (s.duration * 1000.0) as i64)
        .sum();

    Some(anchor_pdt + chrono::Duration::milliseconds(offset_ms))
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hls::cue::AdBreak;
    use m3u8_rs::{ExtTag, MediaSegment};

    fn make_segment(duration: f32) -> MediaSegment {
        MediaSegment {
            uri: "seg.ts".to_string(),
            duration,
            ..Default::default()
        }
    }

    fn make_segment_with_tags(duration: f32, tags: Vec<(&str, Option<&str>)>) -> MediaSegment {
        let unknown_tags = tags
            .into_iter()
            .map(|(tag, rest)| ExtTag {
                tag: tag.to_string(),
                rest: rest.map(|s| s.to_string()),
            })
            .collect();
        MediaSegment {
            uri: "seg.ts".to_string(),
            duration,
            unknown_tags,
            ..Default::default()
        }
    }

    fn make_playlist(segments: Vec<MediaSegment>) -> MediaPlaylist {
        MediaPlaylist {
            segments,
            ..Default::default()
        }
    }

    #[test]
    fn ensure_pdt_synthesizes_when_missing() {
        let mut playlist = make_playlist(vec![
            make_segment(6.0),
            make_segment(6.0),
            make_segment(6.0),
        ]);

        ensure_program_date_time(&mut playlist);

        // All segments should have PDT set
        assert!(playlist.segments[0].program_date_time.is_some());
        assert!(playlist.segments[1].program_date_time.is_some());
        assert!(playlist.segments[2].program_date_time.is_some());

        // PDT should advance by segment duration
        let pdt0 = playlist.segments[0].program_date_time.unwrap();
        let pdt1 = playlist.segments[1].program_date_time.unwrap();
        let pdt2 = playlist.segments[2].program_date_time.unwrap();

        assert_eq!((pdt1 - pdt0).num_milliseconds(), 6000);
        assert_eq!((pdt2 - pdt1).num_milliseconds(), 6000);
    }

    #[test]
    fn ensure_pdt_preserves_existing() {
        let existing_pdt = FixedOffset::east_opt(0)
            .unwrap()
            .with_ymd_and_hms(2025, 6, 1, 12, 0, 0)
            .unwrap();

        let mut seg = make_segment(10.0);
        seg.program_date_time = Some(existing_pdt);

        let mut playlist = make_playlist(vec![seg, make_segment(10.0)]);
        ensure_program_date_time(&mut playlist);

        // First segment PDT must be unchanged
        assert_eq!(playlist.segments[0].program_date_time, Some(existing_pdt));
        // Second segment PDT must remain None (we don't back-fill)
        assert!(playlist.segments[1].program_date_time.is_none());
    }

    #[test]
    fn inject_single_interstitial() {
        let mut playlist = make_playlist(vec![
            make_segment(10.0),
            make_segment_with_tags(10.0, vec![("X-CUE-OUT", Some("30"))]),
            make_segment(10.0),
            make_segment_with_tags(10.0, vec![("X-CUE-IN", None)]),
            make_segment(10.0),
        ]);

        ensure_program_date_time(&mut playlist);
        let ad_breaks = vec![AdBreak {
            start_index: 1,
            end_index: 3,
            duration: 30.0,
        }];

        inject_interstitials(&mut playlist, &ad_breaks, "sess-1", "http://localhost:3000");

        let dr = playlist.segments[1]
            .daterange
            .as_ref()
            .expect("DateRange should be set on break-start segment");

        assert_eq!(dr.id, "ad-break-0");
        assert_eq!(dr.class, Some("com.apple.hls.interstitial".to_string()));
        assert_eq!(dr.duration, Some(30.0));
    }

    #[test]
    fn inject_multiple_interstitials() {
        let mut playlist = make_playlist(vec![
            make_segment(10.0),
            make_segment_with_tags(10.0, vec![("X-CUE-OUT", Some("30"))]),
            make_segment_with_tags(10.0, vec![("X-CUE-IN", None)]),
            make_segment(10.0),
            make_segment_with_tags(10.0, vec![("X-CUE-OUT", Some("60"))]),
            make_segment_with_tags(10.0, vec![("X-CUE-IN", None)]),
        ]);

        ensure_program_date_time(&mut playlist);
        let ad_breaks = vec![
            AdBreak {
                start_index: 1,
                end_index: 2,
                duration: 30.0,
            },
            AdBreak {
                start_index: 4,
                end_index: 5,
                duration: 60.0,
            },
        ];

        inject_interstitials(&mut playlist, &ad_breaks, "sess-2", "http://localhost:3000");

        assert!(playlist.segments[1].daterange.is_some());
        assert!(playlist.segments[4].daterange.is_some());

        assert_eq!(
            playlist.segments[1].daterange.as_ref().unwrap().id,
            "ad-break-0"
        );
        assert_eq!(
            playlist.segments[4].daterange.as_ref().unwrap().id,
            "ad-break-1"
        );
        assert_eq!(
            playlist.segments[4].daterange.as_ref().unwrap().duration,
            Some(60.0)
        );
    }

    #[test]
    fn inject_removes_cue_tags() {
        let mut playlist = make_playlist(vec![
            make_segment(10.0),
            make_segment_with_tags(
                10.0,
                vec![("X-CUE-OUT", Some("30")), ("X-CUE-OUT-CONT", Some("5/30"))],
            ),
            make_segment_with_tags(10.0, vec![("X-CUE-IN", None)]),
            make_segment(10.0),
        ]);

        ensure_program_date_time(&mut playlist);
        let ad_breaks = vec![AdBreak {
            start_index: 1,
            end_index: 2,
            duration: 30.0,
        }];

        inject_interstitials(&mut playlist, &ad_breaks, "sess-3", "http://localhost:3000");

        // All CUE tags should be gone
        for seg in &playlist.segments {
            for tag in &seg.unknown_tags {
                assert!(
                    !is_cue_tag(&tag.tag),
                    "CUE tag {} should have been removed",
                    tag.tag
                );
            }
        }
    }

    #[test]
    fn daterange_has_correct_x_attributes() {
        let mut playlist = make_playlist(vec![
            make_segment(10.0),
            make_segment_with_tags(10.0, vec![("X-CUE-OUT", Some("30"))]),
            make_segment_with_tags(10.0, vec![("X-CUE-IN", None)]),
        ]);

        ensure_program_date_time(&mut playlist);
        let ad_breaks = vec![AdBreak {
            start_index: 1,
            end_index: 2,
            duration: 30.0,
        }];

        inject_interstitials(
            &mut playlist,
            &ad_breaks,
            "my-sess",
            "https://ritcher.example.com",
        );

        let dr = playlist.segments[1].daterange.as_ref().unwrap();
        let x = dr.x_prefixed.as_ref().expect("x_prefixed should be Some");

        // X-ASSET-LIST should be present and contain session_id, break_id, duration
        let asset_list = x.get("X-ASSET-LIST").expect("X-ASSET-LIST should exist");
        let url = asset_list.as_str();
        assert!(url.contains("my-sess"), "URL should contain session_id");
        assert!(url.contains("/asset-list/0"), "URL should contain break_id");
        assert!(url.contains("dur=30"), "URL should contain duration");

        // X-RESUME-OFFSET should be unquoted "0"
        let resume = x
            .get("X-RESUME-OFFSET")
            .expect("X-RESUME-OFFSET should exist");
        assert_eq!(resume.as_unquoted(), Some("0"));

        // X-RESTRICT should be quoted "SKIP,JUMP"
        let restrict = x.get("X-RESTRICT").expect("X-RESTRICT should exist");
        assert_eq!(restrict.as_quoted(), Some("SKIP,JUMP"));
    }

    #[test]
    fn asset_list_url_format() {
        let mut playlist = make_playlist(vec![
            make_segment(6.0),
            make_segment_with_tags(6.0, vec![("X-CUE-OUT", Some("30"))]),
            make_segment_with_tags(6.0, vec![("X-CUE-IN", None)]),
        ]);

        ensure_program_date_time(&mut playlist);
        let ad_breaks = vec![AdBreak {
            start_index: 1,
            end_index: 2,
            duration: 30.0,
        }];

        inject_interstitials(
            &mut playlist,
            &ad_breaks,
            "test-session",
            "https://stitcher.example.com",
        );

        let dr = playlist.segments[1].daterange.as_ref().unwrap();
        let asset_list_url = dr
            .x_prefixed
            .as_ref()
            .unwrap()
            .get("X-ASSET-LIST")
            .unwrap()
            .as_str();

        assert_eq!(
            asset_list_url,
            "https://stitcher.example.com/stitch/test-session/asset-list/0?dur=30"
        );
    }

    #[test]
    fn compute_pdt_at_accumulates_correctly() {
        let base = FixedOffset::east_opt(0)
            .unwrap()
            .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
            .unwrap();

        let mut seg0 = make_segment(10.0);
        seg0.program_date_time = Some(base);

        let playlist = make_playlist(vec![seg0, make_segment(10.0), make_segment(10.0)]);

        let pdt2 = compute_pdt_at(&playlist, 2).unwrap();
        assert_eq!((pdt2 - base).num_seconds(), 20);
    }
}
