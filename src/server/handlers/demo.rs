use axum::{
    extract::Query,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use std::fmt::Write;
use tracing::info;

/// Base URL for Mux Big Buck Bunny test stream segments
const MUX_BASE: &str = "https://test-streams.mux.dev/x36xhzz/url_0";
/// Mux segment filename
const MUX_SEGMENT: &str = "193039199_mp4_h264_aac_hd_7.ts";
/// First Mux segment index
const MUX_START_INDEX: u32 = 462;
/// Duration of each segment in seconds
const SEGMENT_DURATION: f32 = 10.0;
/// Duration of each ad break in seconds (matches DemoAdProvider: 10 × 1s segments)
const BREAK_DURATION: u32 = 10;
/// Number of placeholder content segments per ad break (10s / 10s = 1)
const BREAK_SEGMENTS: u32 = 1;

/// Query parameters for configurable demo endpoints
#[derive(Debug, Deserialize)]
pub struct DemoParams {
    /// Number of ad breaks (1-5, default: 1)
    breaks: Option<u8>,
    /// Seconds of content between ad breaks (10, 15, 20; default: 15)
    interval: Option<u8>,
}

impl DemoParams {
    /// Validated number of breaks, clamped to 1..=5
    fn num_breaks(&self) -> u8 {
        self.breaks.unwrap_or(1).clamp(1, 5)
    }

    /// Validated interval in seconds, snapped to nearest allowed value
    fn interval_secs(&self) -> u8 {
        match self.interval.unwrap_or(15) {
            0..=12 => 10,
            13..=17 => 15,
            _ => 20,
        }
    }
}

/// Build a Mux segment URL for the given index
fn mux_segment_url(index: u32) -> String {
    format!("{}/url_{}/{}", MUX_BASE, index, MUX_SEGMENT)
}

/// Build a dynamic HLS demo playlist with configurable ad breaks
///
/// Generates a VOD playlist using Mux Big Buck Bunny segments with
/// SCTE-35 CUE-OUT/CUE-IN markers at configurable intervals.
///
/// # Arguments
/// * `num_breaks` - Number of ad breaks (1-5)
/// * `interval_secs` - Seconds of content before each break (10, 15, 20)
fn build_demo_hls(num_breaks: u8, interval_secs: u8) -> String {
    let segs_per_interval = (interval_secs as f32 / SEGMENT_DURATION) as u32;
    let mut seg_idx = MUX_START_INDEX;
    let mut playlist = String::with_capacity(4096);

    // Header
    let _ = writeln!(playlist, "#EXTM3U");
    let _ = writeln!(playlist, "#EXT-X-VERSION:3");
    let _ = writeln!(playlist, "#EXT-X-TARGETDURATION:10");
    let _ = writeln!(playlist, "#EXT-X-MEDIA-SEQUENCE:0");
    let _ = writeln!(
        playlist,
        "#EXT-X-PROGRAM-DATE-TIME:2026-01-01T00:00:00.000Z"
    );
    let _ = writeln!(playlist);

    for break_num in 0..num_breaks {
        // Content segments before this break
        for _ in 0..segs_per_interval {
            let _ = writeln!(playlist, "#EXTINF:{:.1},", SEGMENT_DURATION);
            let _ = writeln!(playlist, "{}", mux_segment_url(seg_idx));
            seg_idx += 1;
        }
        let _ = writeln!(playlist);

        // CUE-OUT: start of ad break
        let _ = writeln!(playlist, "#EXT-X-CUE-OUT:{}", BREAK_DURATION);

        // Placeholder segments within the ad break (replaced by the stitcher).
        // Use the LAST content segment as placeholder — do NOT advance seg_idx,
        // so content resumes seamlessly after the ad break.
        let placeholder_idx = seg_idx.saturating_sub(1);
        for cont_idx in 0..BREAK_SEGMENTS {
            if cont_idx > 0 {
                let elapsed = cont_idx * (SEGMENT_DURATION as u32);
                let _ = writeln!(
                    playlist,
                    "#EXT-X-CUE-OUT-CONT:{}/{}",
                    elapsed, BREAK_DURATION
                );
            }
            let _ = writeln!(playlist, "#EXTINF:{:.1},", SEGMENT_DURATION);
            let _ = writeln!(playlist, "{}", mux_segment_url(placeholder_idx));
        }

        // CUE-IN: end of ad break
        let _ = writeln!(playlist, "#EXT-X-CUE-IN");
        let _ = writeln!(playlist);

        info!(
            "Demo HLS: ad break {} at segment index {}",
            break_num + 1,
            seg_idx - BREAK_SEGMENTS
        );
    }

    // Trailing content after the last break
    let trailing = 3u32;
    for _ in 0..trailing {
        let _ = writeln!(playlist, "#EXTINF:{:.1},", SEGMENT_DURATION);
        let _ = writeln!(playlist, "{}", mux_segment_url(seg_idx));
        seg_idx += 1;
    }

    let _ = writeln!(playlist);
    let _ = writeln!(playlist, "#EXT-X-ENDLIST");

    playlist
}

/// Build a dynamic DASH demo manifest with configurable ad breaks
///
/// Generates a static DASH MPD using Mux Big Buck Bunny segments with
/// SCTE-35 EventStream signals at configurable intervals.
fn build_demo_mpd(num_breaks: u8, interval_secs: u8) -> String {
    let segs_per_interval = interval_secs as u32 / SEGMENT_DURATION as u32;
    let mut seg_start = MUX_START_INDEX;
    let mut mpd = String::with_capacity(4096);

    // Calculate total duration
    let content_per_break = interval_secs as u32 + BREAK_DURATION;
    let total_duration = num_breaks as u32 * content_per_break + 30; // +30s trailing

    // MPD header
    let _ = writeln!(mpd, r#"<?xml version="1.0" encoding="UTF-8"?>"#);
    let _ = writeln!(
        mpd,
        r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT{}S" minBufferTime="PT2S" profiles="urn:mpeg:dash:profile:isoff-live:2011">"#,
        total_duration
    );

    for break_num in 0..num_breaks {
        let period_duration = interval_secs as u32 + BREAK_DURATION;
        let event_time = interval_secs as u32; // Event at end of content interval

        // Content period with EventStream signaling the ad break
        let _ = writeln!(
            mpd,
            r#"  <Period id="content-{}" duration="PT{}S">"#,
            break_num + 1,
            period_duration
        );
        let _ = writeln!(mpd, r#"    <BaseURL>{}/</BaseURL>"#, MUX_BASE);

        // Video AdaptationSet
        let _ = writeln!(
            mpd,
            r#"    <AdaptationSet id="1" contentType="video" mimeType="video/mp2t">"#
        );
        let _ = writeln!(
            mpd,
            r#"      <Representation id="video" bandwidth="800000" codecs="avc1.64001f">"#
        );
        let _ = writeln!(
            mpd,
            r#"        <SegmentTemplate media="url_$Number$/{}" timescale="1" duration="10" startNumber="{}"/>"#,
            MUX_SEGMENT, seg_start
        );
        let _ = writeln!(mpd, r#"      </Representation>"#);
        let _ = writeln!(mpd, r#"    </AdaptationSet>"#);

        // Audio AdaptationSet
        let _ = writeln!(
            mpd,
            r#"    <AdaptationSet id="2" contentType="audio" mimeType="audio/mp4" lang="en">"#
        );
        let _ = writeln!(
            mpd,
            r#"      <Representation id="audio" bandwidth="128000" codecs="mp4a.40.2">"#
        );
        let _ = writeln!(
            mpd,
            r#"        <SegmentTemplate media="url_$Number$/{}" timescale="1" duration="10" startNumber="{}"/>"#,
            MUX_SEGMENT, seg_start
        );
        let _ = writeln!(mpd, r#"      </Representation>"#);
        let _ = writeln!(mpd, r#"    </AdaptationSet>"#);

        // SCTE-35 EventStream
        let _ = writeln!(
            mpd,
            r#"    <EventStream schemeIdUri="urn:scte:scte35:2013:xml" timescale="1">"#
        );
        let _ = writeln!(
            mpd,
            r#"      <Event presentationTime="{}" duration="{}" id="ad-{}">"#,
            event_time,
            BREAK_DURATION,
            break_num + 1
        );
        let _ = writeln!(
            mpd,
            r#"        <scte35:SpliceInfoSection xmlns:scte35="http://www.scte.org/schemas/35/2016">"#
        );
        let _ = writeln!(
            mpd,
            r#"          <scte35:SpliceInsert spliceEventId="{}" outOfNetworkIndicator="true">"#,
            100 + break_num
        );
        let _ = writeln!(
            mpd,
            r#"            <scte35:BreakDuration autoReturn="true" duration="{}"/>"#,
            BREAK_DURATION
        );
        let _ = writeln!(mpd, r#"          </scte35:SpliceInsert>"#);
        let _ = writeln!(mpd, r#"        </scte35:SpliceInfoSection>"#);
        let _ = writeln!(mpd, r#"      </Event>"#);
        let _ = writeln!(mpd, r#"    </EventStream>"#);

        let _ = writeln!(mpd, r#"  </Period>"#);

        // Only advance by content segments — break segments are placeholders
        // that get replaced by the stitcher, so they don't consume content indices
        seg_start += segs_per_interval;
    }

    // Trailing content period (30s)
    let _ = writeln!(mpd, r#"  <Period id="content-trailing" duration="PT30S">"#);
    let _ = writeln!(mpd, r#"    <BaseURL>{}/</BaseURL>"#, MUX_BASE);
    let _ = writeln!(
        mpd,
        r#"    <AdaptationSet id="1" contentType="video" mimeType="video/mp2t">"#
    );
    let _ = writeln!(
        mpd,
        r#"      <Representation id="video" bandwidth="800000" codecs="avc1.64001f">"#
    );
    let _ = writeln!(
        mpd,
        r#"        <SegmentTemplate media="url_$Number$/{}" timescale="1" duration="10" startNumber="{}"/>"#,
        MUX_SEGMENT, seg_start
    );
    let _ = writeln!(mpd, r#"      </Representation>"#);
    let _ = writeln!(mpd, r#"    </AdaptationSet>"#);
    let _ = writeln!(
        mpd,
        r#"    <AdaptationSet id="2" contentType="audio" mimeType="audio/mp4" lang="en">"#
    );
    let _ = writeln!(
        mpd,
        r#"      <Representation id="audio" bandwidth="128000" codecs="mp4a.40.2">"#
    );
    let _ = writeln!(
        mpd,
        r#"        <SegmentTemplate media="url_$Number$/{}" timescale="1" duration="10" startNumber="{}"/>"#,
        MUX_SEGMENT, seg_start
    );
    let _ = writeln!(mpd, r#"      </Representation>"#);
    let _ = writeln!(mpd, r#"    </AdaptationSet>"#);
    let _ = writeln!(mpd, r#"  </Period>"#);

    let _ = writeln!(mpd, r#"</MPD>"#);

    mpd
}

/// Demo HLS playlist endpoint with configurable ad breaks
///
/// Serves a synthetic HLS media playlist using Mux Big Buck Bunny segments
/// with SCTE-35 CUE-OUT/CUE-IN markers at configurable positions.
///
/// # Query Parameters
/// * `breaks` — Number of ad breaks, 1-5 (default: 1)
/// * `interval` — Seconds between breaks: 10, 15, or 20 (default: 15)
///
/// # Usage
/// ```text
/// GET /demo/playlist.m3u8                      → 1 break, 15s interval
/// GET /demo/playlist.m3u8?breaks=3&interval=20 → 3 breaks, 20s apart
/// ```
pub async fn serve_demo_playlist(Query(params): Query<DemoParams>) -> Response {
    let num_breaks = params.num_breaks();
    let interval = params.interval_secs();

    info!(
        "Serving demo HLS playlist: {} breaks, {}s interval",
        num_breaks, interval
    );

    let playlist = build_demo_hls(num_breaks, interval);
    let total_segs = num_breaks as u32 * ((interval as u32 / 10) + BREAK_SEGMENTS) + 3;

    info!(
        "Demo playlist: {} segments, {} ad breaks ({}s each) at {}s intervals",
        total_segs, num_breaks, BREAK_DURATION, interval
    );

    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/vnd.apple.mpegurl")],
        playlist,
    )
        .into_response()
}

/// Demo DASH manifest endpoint with configurable ad breaks
///
/// Serves a synthetic DASH MPD using Mux Big Buck Bunny segments with
/// SCTE-35 EventStream signals at configurable positions.
///
/// # Query Parameters
/// Same as the HLS endpoint: `breaks` (1-5) and `interval` (10, 15, 20).
pub async fn serve_demo_manifest(Query(params): Query<DemoParams>) -> Response {
    let num_breaks = params.num_breaks();
    let interval = params.interval_secs();

    info!(
        "Serving demo DASH manifest: {} breaks, {}s interval",
        num_breaks, interval
    );

    let manifest = build_demo_mpd(num_breaks, interval);

    info!(
        "Demo manifest: {} content periods + trailing, {} SCTE-35 signals",
        num_breaks, num_breaks
    );

    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/dash+xml")],
        manifest,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_demo_params_defaults() {
        let params = DemoParams {
            breaks: None,
            interval: None,
        };
        assert_eq!(params.num_breaks(), 1);
        assert_eq!(params.interval_secs(), 15);
    }

    #[test]
    fn test_demo_params_clamping() {
        // Breaks clamped to 1..=5
        let p = DemoParams {
            breaks: Some(0),
            interval: None,
        };
        assert_eq!(p.num_breaks(), 1);

        let p = DemoParams {
            breaks: Some(10),
            interval: None,
        };
        assert_eq!(p.num_breaks(), 5);

        // Interval snapping
        let p = DemoParams {
            breaks: None,
            interval: Some(5),
        };
        assert_eq!(p.interval_secs(), 10);

        let p = DemoParams {
            breaks: None,
            interval: Some(14),
        };
        assert_eq!(p.interval_secs(), 15);

        let p = DemoParams {
            breaks: None,
            interval: Some(25),
        };
        assert_eq!(p.interval_secs(), 20);
    }

    #[test]
    fn test_build_demo_hls_single_break() {
        let playlist = build_demo_hls(1, 15);

        // Should contain header
        assert!(playlist.contains("#EXTM3U"));
        assert!(playlist.contains("#EXT-X-TARGETDURATION:10"));
        assert!(playlist.contains("#EXT-X-PROGRAM-DATE-TIME:"));

        // Should have exactly 1 CUE-OUT and 1 CUE-IN
        assert_eq!(
            playlist.matches("#EXT-X-CUE-OUT:10").count(),
            1,
            "Expected 1 CUE-OUT"
        );
        assert_eq!(
            playlist.matches("#EXT-X-CUE-IN").count(),
            1,
            "Expected 1 CUE-IN"
        );

        // 15s interval = 1 content seg (10s rounded), then 1 break seg, then 3 trailing
        // 15/10 = 1.5 → truncated to 1 content segment before break
        let seg_count = playlist.matches("#EXTINF:").count();
        // 1 content + 1 break + 3 trailing = 5 segments
        assert_eq!(seg_count, 5, "Expected 5 segments");

        assert!(playlist.contains("#EXT-X-ENDLIST"));
    }

    #[test]
    fn test_build_demo_hls_five_breaks_20s() {
        let playlist = build_demo_hls(5, 20);

        // 5 CUE-OUT/CUE-IN pairs
        assert_eq!(playlist.matches("#EXT-X-CUE-OUT:10").count(), 5);
        assert_eq!(playlist.matches("#EXT-X-CUE-IN").count(), 5);

        // 20s interval = 2 content segs per break, 1 break seg per break, 3 trailing
        // 5 * (2 + 1) + 3 = 18 segments
        let seg_count = playlist.matches("#EXTINF:").count();
        assert_eq!(seg_count, 18, "Expected 18 segments for 5 breaks @ 20s");
    }

    #[test]
    fn test_build_demo_hls_segment_urls_are_valid() {
        let playlist = build_demo_hls(1, 10);

        // All segments should reference Mux test streams
        for line in playlist.lines() {
            if line.starts_with("https://") {
                assert!(
                    line.contains("test-streams.mux.dev"),
                    "Unexpected URL: {}",
                    line
                );
                assert!(line.ends_with(".ts"), "URL should end with .ts: {}", line);
            }
        }
    }

    #[test]
    fn test_build_demo_mpd_single_break() {
        let mpd = build_demo_mpd(1, 15);

        assert!(mpd.contains("<?xml version"));
        assert!(mpd.contains("<MPD"));

        // Should have content period + trailing period
        assert!(mpd.contains(r#"id="content-1""#));
        assert!(mpd.contains(r#"id="content-trailing""#));

        // Should have 1 SCTE-35 event
        assert_eq!(
            mpd.matches("urn:scte:scte35:2013:xml").count(),
            1,
            "Expected 1 EventStream"
        );
        assert!(mpd.contains(r#"id="ad-1""#));

        assert!(mpd.contains("</MPD>"));
    }

    #[test]
    fn test_build_demo_mpd_five_breaks() {
        let mpd = build_demo_mpd(5, 20);

        // 5 content periods + 1 trailing
        for i in 1..=5 {
            assert!(
                mpd.contains(&format!(r#"id="content-{}""#, i)),
                "Missing content period {}",
                i
            );
        }
        assert!(mpd.contains(r#"id="content-trailing""#));

        // 5 EventStreams
        assert_eq!(mpd.matches("urn:scte:scte35:2013:xml").count(), 5);

        // 5 ad events
        for i in 1..=5 {
            assert!(
                mpd.contains(&format!(r#"id="ad-{}""#, i)),
                "Missing ad event {}",
                i
            );
        }
    }

    #[test]
    fn test_build_demo_mpd_segment_start_numbers_increment() {
        let mpd = build_demo_mpd(2, 10);

        // First period starts at 462
        assert!(mpd.contains(r#"startNumber="462""#));

        // Second period: 1 content seg (10s/10s), break segments don't advance
        // So second period starts at 462 + 1 = 463
        assert!(mpd.contains(r#"startNumber="463""#));
    }
}
