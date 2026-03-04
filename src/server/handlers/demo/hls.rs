use axum::{
    extract::Query,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use std::fmt::Write;
use tracing::info;

use super::{
    BREAK_DURATION, BREAK_SEGMENTS, DemoParams, MUX_BASE, MUX_START_INDEX, SEGMENT_DURATION,
    mux_segment_url,
};

/// Build a dynamic HLS demo playlist with configurable ad breaks
///
/// Generates a VOD playlist using Mux Big Buck Bunny segments with
/// SCTE-35 CUE-OUT/CUE-IN markers at configurable intervals.
///
/// # Arguments
/// * `num_breaks` - Number of ad breaks (1-5)
/// * `interval_secs` - Seconds of content before each break (10, 20, 30)
fn build_demo_hls(num_breaks: u8, interval_secs: u8) -> String {
    // interval_secs (u8, max 30) / SEGMENT_DURATION (10.0) yields <= 3.0; safe to truncate.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
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
        // Use the LAST content segment as placeholder -- do NOT advance seg_idx,
        // so content resumes seamlessly after the ad break.
        let placeholder_idx = seg_idx.saturating_sub(1);
        for cont_idx in 0..BREAK_SEGMENTS {
            if cont_idx > 0 {
                // SEGMENT_DURATION is 10.0; truncation to u32 is lossless.
                #[allow(clippy::cast_possible_truncation)]
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

/// Demo HLS playlist endpoint with configurable ad breaks
///
/// Serves a synthetic HLS media playlist using Mux Big Buck Bunny segments
/// with SCTE-35 CUE-OUT/CUE-IN markers at configurable positions.
///
/// # Query Parameters
/// * `breaks` -- Number of ad breaks, 1-5 (default: 1)
/// * `interval` -- Seconds between breaks: 10, 20, or 30 (default: 10)
///
/// # Usage
/// ```text
/// GET /demo/playlist.m3u8                      -> 1 break, 15s interval
/// GET /demo/playlist.m3u8?breaks=3&interval=30 -> 3 breaks, 30s apart
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

// -- LL-HLS Demo -----------------------------------------------------------

/// LL-HLS part target duration in seconds
const LL_HLS_PART_TARGET: f64 = 0.33334;
/// Number of partial segments per full segment in LL-HLS demo
const LL_HLS_PARTS_PER_SEG: u32 = 3;

/// Write a single LL-HLS segment with its partial segments to the playlist buffer.
///
/// Each full segment has `LL_HLS_PARTS_PER_SEG` parts. The first part of each
/// segment is marked `INDEPENDENT=YES` (required by LL-HLS spec for switching).
fn write_ll_hls_segment(playlist: &mut String, seg_idx: u32) {
    for part in 0..LL_HLS_PARTS_PER_SEG {
        if part == 0 {
            let _ = writeln!(
                playlist,
                "#EXT-X-PART:DURATION={:.5},URI=\"{}/seg{}.{}.mp4\",INDEPENDENT=YES",
                LL_HLS_PART_TARGET, MUX_BASE, seg_idx, part
            );
        } else {
            let _ = writeln!(
                playlist,
                "#EXT-X-PART:DURATION={:.5},URI=\"{}/seg{}.{}.mp4\"",
                LL_HLS_PART_TARGET, MUX_BASE, seg_idx, part
            );
        }
    }
    let seg_duration = LL_HLS_PART_TARGET * LL_HLS_PARTS_PER_SEG as f64;
    let _ = writeln!(playlist, "#EXTINF:{:.5},", seg_duration);
    let _ = writeln!(playlist, "{}", mux_segment_url(seg_idx));
}

/// Build a synthetic LL-HLS demo playlist with configurable ad breaks
///
/// Generates a live-like media playlist with Low-Latency HLS tags:
/// - `EXT-X-SERVER-CONTROL` (blocking reload, skip, part hold-back)
/// - `EXT-X-PART-INF` (partial segment target duration)
/// - `EXT-X-PART` (partial segments, 3 per full segment)
/// - `EXT-X-PRELOAD-HINT` (next expected partial segment)
/// - `EXT-X-RENDITION-REPORT` (alternative rendition status)
///
/// Content segments use Mux Big Buck Bunny test stream URLs. Partial segment
/// URIs are synthetic (not playable individually) but structurally correct
/// for testing the stitcher's LL-HLS URI rewriting pipeline.
///
/// # Arguments
/// * `num_breaks` - Number of ad breaks (1-5)
/// * `interval_secs` - Seconds of content before each break (10, 20, 30)
fn build_demo_ll_hls(num_breaks: u8, interval_secs: u8) -> String {
    // Each full segment ~ 1s (3 parts x 0.33334s)
    let segs_per_interval = interval_secs as u32;
    let mut seg_idx = MUX_START_INDEX;
    let mut playlist = String::with_capacity(8192);

    // LL-HLS header
    let _ = writeln!(playlist, "#EXTM3U");
    let _ = writeln!(playlist, "#EXT-X-VERSION:6");
    let _ = writeln!(playlist, "#EXT-X-TARGETDURATION:4");
    let _ = writeln!(
        playlist,
        "#EXT-X-SERVER-CONTROL:CAN-BLOCK-RELOAD=YES,PART-HOLD-BACK=1.0,CAN-SKIP-UNTIL=12.0"
    );
    let _ = writeln!(
        playlist,
        "#EXT-X-PART-INF:PART-TARGET={:.5}",
        LL_HLS_PART_TARGET
    );
    let _ = writeln!(playlist, "#EXT-X-MEDIA-SEQUENCE:{}", MUX_START_INDEX);
    let _ = writeln!(
        playlist,
        "#EXT-X-PROGRAM-DATE-TIME:2026-01-01T00:00:00.000Z"
    );
    let _ = writeln!(playlist);

    for break_num in 0..num_breaks {
        // Content segments with partial segments before each break
        for _ in 0..segs_per_interval {
            write_ll_hls_segment(&mut playlist, seg_idx);
            seg_idx += 1;
        }
        let _ = writeln!(playlist);

        // CUE-OUT: start of ad break
        let _ = writeln!(playlist, "#EXT-X-CUE-OUT:{}", BREAK_DURATION);

        // Placeholder segment within the ad break (replaced by the stitcher).
        // Use the LAST content segment as placeholder -- do NOT advance seg_idx.
        let placeholder_idx = seg_idx.saturating_sub(1);
        let _ = writeln!(playlist, "#EXTINF:{:.1},", SEGMENT_DURATION);
        let _ = writeln!(playlist, "{}", mux_segment_url(placeholder_idx));

        // CUE-IN: end of ad break
        let _ = writeln!(playlist, "#EXT-X-CUE-IN");
        let _ = writeln!(playlist);

        info!(
            "Demo LL-HLS: ad break {} at segment index {}",
            break_num + 1,
            seg_idx
        );
    }

    // Trailing content after the last break
    for _ in 0..3u32 {
        write_ll_hls_segment(&mut playlist, seg_idx);
        seg_idx += 1;
    }

    let _ = writeln!(playlist);

    // LL-HLS ending tags: preload hint for next partial + rendition report
    let _ = writeln!(
        playlist,
        "#EXT-X-PRELOAD-HINT:TYPE=PART,URI=\"{}/seg{}.0.mp4\"",
        MUX_BASE, seg_idx
    );
    let _ = writeln!(
        playlist,
        "#EXT-X-RENDITION-REPORT:URI=\"alt-playlist.m3u8\",LAST-MSN={},LAST-PART=2",
        seg_idx - 1
    );

    playlist
}

/// Demo LL-HLS playlist endpoint with configurable ad breaks
///
/// Serves a synthetic Low-Latency HLS media playlist with LL-HLS tags
/// (`SERVER-CONTROL`, `PART-INF`, `PART`, `PRELOAD-HINT`, `RENDITION-REPORT`)
/// and SCTE-35 CUE-OUT/CUE-IN markers at configurable positions.
///
/// # Query Parameters
/// * `breaks` -- Number of ad breaks, 1-5 (default: 1)
/// * `interval` -- Seconds between breaks: 10, 20, or 30 (default: 10)
///
/// # Usage
/// ```text
/// GET /demo/ll-hls/playlist.m3u8                      -> 1 break, 15s interval
/// GET /demo/ll-hls/playlist.m3u8?breaks=3&interval=30 -> 3 breaks, 30s apart
/// ```
pub async fn serve_demo_ll_hls_playlist(Query(params): Query<DemoParams>) -> Response {
    let num_breaks = params.num_breaks();
    let interval = params.interval_secs();

    info!(
        "Serving demo LL-HLS playlist: {} breaks, {}s interval",
        num_breaks, interval
    );

    let playlist = build_demo_ll_hls(num_breaks, interval);

    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/vnd.apple.mpegurl")],
        playlist,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_demo_hls_single_break() {
        let playlist = build_demo_hls(1, 10);

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

        // 10s interval = 1 content seg, then 1 break seg, then 3 trailing
        let seg_count = playlist.matches("#EXTINF:").count();
        // 1 content + 1 break + 3 trailing = 5 segments
        assert_eq!(seg_count, 5, "Expected 5 segments");

        assert!(playlist.contains("#EXT-X-ENDLIST"));
    }

    #[test]
    fn test_build_demo_hls_five_breaks_30s() {
        let playlist = build_demo_hls(5, 30);

        // 5 CUE-OUT/CUE-IN pairs
        assert_eq!(playlist.matches("#EXT-X-CUE-OUT:10").count(), 5);
        assert_eq!(playlist.matches("#EXT-X-CUE-IN").count(), 5);

        // 30s interval = 3 content segs per break, 1 break seg per break, 3 trailing
        // 5 * (3 + 1) + 3 = 23 segments
        let seg_count = playlist.matches("#EXTINF:").count();
        assert_eq!(seg_count, 23, "Expected 23 segments for 5 breaks @ 30s");
    }

    #[test]
    fn test_build_demo_hls_segment_urls_are_valid() {
        let playlist = build_demo_hls(1, 10);

        // All HLS segments should reference Mux test streams (MPEG-TS)
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

    // -- LL-HLS demo tests --

    #[test]
    fn test_build_demo_ll_hls_has_ll_hls_tags() {
        let playlist = build_demo_ll_hls(1, 10);

        // Must have all LL-HLS header tags
        assert!(
            playlist.contains("#EXT-X-SERVER-CONTROL:CAN-BLOCK-RELOAD=YES"),
            "Missing SERVER-CONTROL"
        );
        assert!(
            playlist.contains("#EXT-X-PART-INF:PART-TARGET="),
            "Missing PART-INF"
        );
        assert!(playlist.contains("#EXT-X-VERSION:6"), "Missing VERSION:6");

        // Must have partial segments
        assert!(
            playlist.contains("#EXT-X-PART:DURATION="),
            "Missing EXT-X-PART tags"
        );

        // Must have preload hint and rendition report
        assert!(
            playlist.contains("#EXT-X-PRELOAD-HINT:TYPE=PART"),
            "Missing PRELOAD-HINT"
        );
        assert!(
            playlist.contains("#EXT-X-RENDITION-REPORT:URI="),
            "Missing RENDITION-REPORT"
        );

        // Must have CUE markers
        assert_eq!(
            playlist.matches("#EXT-X-CUE-OUT:").count(),
            1,
            "Expected 1 CUE-OUT"
        );
        assert_eq!(
            playlist.matches("#EXT-X-CUE-IN").count(),
            1,
            "Expected 1 CUE-IN"
        );
    }

    #[test]
    fn test_build_demo_ll_hls_partial_segment_structure() {
        let playlist = build_demo_ll_hls(1, 10);

        // First part of each segment should be INDEPENDENT=YES
        let independent_count = playlist.matches("INDEPENDENT=YES").count();
        let part_count = playlist.matches("#EXT-X-PART:DURATION=").count();

        // Each segment has 3 parts, 1 is independent
        assert_eq!(
            independent_count * 3,
            part_count,
            "Each segment should have 1 independent part out of 3 (independent={}, parts={})",
            independent_count,
            part_count
        );
    }

    #[test]
    fn test_build_demo_ll_hls_multiple_breaks() {
        let playlist = build_demo_ll_hls(3, 10);

        assert_eq!(playlist.matches("#EXT-X-CUE-OUT:").count(), 3);
        assert_eq!(playlist.matches("#EXT-X-CUE-IN").count(), 3);

        // Should have SERVER-CONTROL exactly once (header-level)
        assert_eq!(playlist.matches("#EXT-X-SERVER-CONTROL:").count(), 1);

        // Should have PART-INF exactly once
        assert_eq!(playlist.matches("#EXT-X-PART-INF:").count(), 1);
    }

    #[test]
    fn test_build_demo_ll_hls_no_endlist() {
        // LL-HLS is live -- no EXT-X-ENDLIST
        let playlist = build_demo_ll_hls(1, 10);
        assert!(
            !playlist.contains("#EXT-X-ENDLIST"),
            "LL-HLS live playlist should not have ENDLIST"
        );
    }
}
