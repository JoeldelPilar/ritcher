use axum::{
    extract::Query,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use std::fmt::Write;
use tracing::info;

use super::{
    BREAK_DURATION, DASH_AUDIO_REP, DASH_BASE, DASH_SEGMENT_DURATION, DASH_START_NUMBER,
    DASH_VIDEO_REP, DemoParams,
};

/// Write the video AdaptationSet XML to the MPD buffer
fn build_video_adaptation_set(mpd: &mut String, seg_start: u32) {
    let _ = writeln!(
        mpd,
        r#"    <AdaptationSet id="1" contentType="video" mimeType="video/mp4">"#
    );
    let _ = writeln!(
        mpd,
        r#"      <Representation id="{}" bandwidth="1013310" codecs="avc1.64001e" width="640" height="360">"#,
        DASH_VIDEO_REP
    );
    let _ = writeln!(
        mpd,
        r#"        <SegmentTemplate initialization="{0}/{0}_0.m4v" media="{0}/{0}_$Number$.m4v" timescale="30" duration="120" startNumber="{1}"/>"#,
        DASH_VIDEO_REP, seg_start
    );
    let _ = writeln!(mpd, r#"      </Representation>"#);
    let _ = writeln!(mpd, r#"    </AdaptationSet>"#);
}

/// Write the audio AdaptationSet XML to the MPD buffer
fn build_audio_adaptation_set(mpd: &mut String, seg_start: u32) {
    let _ = writeln!(
        mpd,
        r#"    <AdaptationSet id="2" contentType="audio" mimeType="audio/mp4" lang="en">"#
    );
    let _ = writeln!(
        mpd,
        r#"      <Representation id="{}" bandwidth="67071" codecs="mp4a.40.5" audioSamplingRate="48000">"#,
        DASH_AUDIO_REP
    );
    let _ = writeln!(
        mpd,
        r#"        <SegmentTemplate initialization="{0}/{0}_0.m4a" media="{0}/{0}_$Number$.m4a" timescale="48000" duration="192512" startNumber="{1}"/>"#,
        DASH_AUDIO_REP, seg_start
    );
    let _ = writeln!(mpd, r#"      </Representation>"#);
    let _ = writeln!(mpd, r#"    </AdaptationSet>"#);
}

/// Write the SCTE-35 EventStream XML to the MPD buffer
fn build_scte35_event_stream(mpd: &mut String, event_time: u32, break_num: u8) {
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
}

/// Build a dynamic DASH demo manifest with configurable ad breaks
///
/// Generates a static DASH MPD using DASH-IF Big Buck Bunny fMP4 segments
/// with SCTE-35 EventStream signals at configurable intervals.
///
/// Uses fMP4 (ISO BMFF) segments from `dash.akamaized.net` -- the standard
/// segment format for DASH (unlike MPEG-TS which is HLS-only).
fn build_demo_mpd(num_breaks: u8, interval_secs: u8) -> String {
    // interval_secs (u8, max 30) / DASH_SEGMENT_DURATION (4.0) yields <= 7.5; safe.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let dash_segs_per_interval = (interval_secs as f32 / DASH_SEGMENT_DURATION).floor() as u32;
    // DASH_SEGMENT_DURATION is 4.0; truncation to u32 is lossless.
    #[allow(clippy::cast_possible_truncation)]
    let actual_interval = dash_segs_per_interval * DASH_SEGMENT_DURATION as u32;
    let mut seg_start = DASH_START_NUMBER;
    let mut mpd = String::with_capacity(4096);

    // Calculate total duration
    let content_per_break = actual_interval + BREAK_DURATION;
    let total_duration = num_breaks as u32 * content_per_break + 28; // +28s trailing (7 x 4s)

    // MPD header
    let _ = writeln!(mpd, r#"<?xml version="1.0" encoding="UTF-8"?>"#);
    let _ = writeln!(
        mpd,
        r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT{}S" minBufferTime="PT2S" profiles="urn:mpeg:dash:profile:isoff-live:2011">"#,
        total_duration
    );

    for break_num in 0..num_breaks {
        let period_duration = actual_interval + BREAK_DURATION;
        let event_time = actual_interval; // Event at end of content interval

        // Content period with EventStream signaling the ad break
        let _ = writeln!(
            mpd,
            r#"  <Period id="content-{}" duration="PT{}S">"#,
            break_num + 1,
            period_duration
        );
        let _ = writeln!(mpd, r#"    <BaseURL>{}/</BaseURL>"#, DASH_BASE);

        build_video_adaptation_set(&mut mpd, seg_start);
        build_audio_adaptation_set(&mut mpd, seg_start);
        build_scte35_event_stream(&mut mpd, event_time, break_num);

        let _ = writeln!(mpd, r#"  </Period>"#);

        // Advance by content segments only -- break segments are placeholders
        seg_start += dash_segs_per_interval;
    }

    // Trailing content period (28s = 7 x 4s segments)
    let _ = writeln!(mpd, r#"  <Period id="content-trailing" duration="PT28S">"#);
    let _ = writeln!(mpd, r#"    <BaseURL>{}/</BaseURL>"#, DASH_BASE);
    build_video_adaptation_set(&mut mpd, seg_start);
    build_audio_adaptation_set(&mut mpd, seg_start);
    let _ = writeln!(mpd, r#"  </Period>"#);

    let _ = writeln!(mpd, r#"</MPD>"#);

    mpd
}

/// Demo DASH manifest endpoint with configurable ad breaks
///
/// Serves a synthetic DASH MPD using Mux Big Buck Bunny segments with
/// SCTE-35 EventStream signals at configurable positions.
///
/// # Query Parameters
/// Same as the HLS endpoint: `breaks` (1-5) and `interval` (10, 20, 30).
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
    fn test_build_demo_mpd_segment_urls_are_fmp4() {
        let mpd = build_demo_mpd(1, 10);

        // DASH segments should reference DASH-IF Akamai CDN (fMP4)
        assert!(
            mpd.contains("dash.akamaized.net"),
            "DASH should use DASH-IF CDN"
        );
        // Must NOT reference Mux MPEG-TS segments
        assert!(
            !mpd.contains("test-streams.mux.dev"),
            "DASH must not use Mux MPEG-TS segments"
        );
        assert!(
            !mpd.contains(".ts\""),
            "DASH must not reference .ts segments"
        );
    }

    #[test]
    fn test_build_demo_mpd_single_break() {
        let mpd = build_demo_mpd(1, 10);

        assert!(mpd.contains("<?xml version"));
        assert!(mpd.contains("<MPD"));

        // Should have content period + trailing period
        assert!(mpd.contains(r#"id="content-1""#));
        assert!(mpd.contains(r#"id="content-trailing""#));

        // Should use fMP4 (video/mp4), not MPEG-TS
        assert!(
            mpd.contains(r#"mimeType="video/mp4""#),
            "DASH must use video/mp4 (fMP4), not video/mp2t"
        );
        assert!(!mpd.contains(r#"mimeType="video/mp2t""#));

        // Should reference DASH-IF Akamai segments
        assert!(mpd.contains("dash.akamaized.net"));
        assert!(mpd.contains(".m4v"));
        assert!(mpd.contains(".m4a"));

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
        let mpd = build_demo_mpd(5, 30);

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

        // First period starts at 1 (DASH_START_NUMBER)
        assert!(mpd.contains(r#"startNumber="1""#));

        // 10s interval / 4s segments = 2 segments per interval
        // Second period starts at 1 + 2 = 3
        assert!(mpd.contains(r#"startNumber="3""#));

        // Trailing period starts at 3 + 2 = 5
        assert!(mpd.contains(r#"startNumber="5""#));
    }

    #[test]
    fn test_build_demo_mpd_has_init_segments() {
        let mpd = build_demo_mpd(1, 10);

        // fMP4 requires initialization segments
        assert!(
            mpd.contains("initialization="),
            "DASH fMP4 must have initialization segments"
        );
        assert!(mpd.contains("_0.m4v"), "Video init segment missing");
        assert!(mpd.contains("_0.m4a"), "Audio init segment missing");
    }
}
