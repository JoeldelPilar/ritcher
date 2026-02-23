use axum::{
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use tracing::info;

/// Demo HLS playlist endpoint for testing the ad insertion pipeline
///
/// Serves a synthetic HLS media playlist that uses segments from a real
/// test stream (Mux public test stream) with SCTE-35 CUE-OUT/CUE-IN
/// markers injected to simulate ad break opportunities.
///
/// Usage:
///   1. Start Ritcher: `DEV_MODE=true cargo run`
///   2. Open VLC or any HLS player
///   3. Point to: http://localhost:3000/stitch/demo/playlist.m3u8?origin=http://localhost:3000/demo/playlist.m3u8
///
/// The stitcher will fetch this demo playlist, detect the CUE markers,
/// and replace the marked segments with ad content.
pub async fn serve_demo_playlist() -> Response {
    info!("Serving demo HLS playlist with CUE markers");

    // Build a synthetic HLS media playlist with real, reachable test segments
    // from the Mux public test stream. Each segment uses a different sub-path
    // (url_462, url_463, etc.) matching the actual Mux stream layout.
    //
    // The CUE markers create a 30-second ad break window at segments 5-7.
    // During stitching, those 3 segments will be replaced by ad content.
    let playlist = r#"#EXTM3U
#EXT-X-VERSION:3
#EXT-X-TARGETDURATION:10
#EXT-X-MEDIA-SEQUENCE:0

#EXTINF:10.0,
https://test-streams.mux.dev/x36xhzz/url_0/url_462/193039199_mp4_h264_aac_hd_7.ts
#EXTINF:10.0,
https://test-streams.mux.dev/x36xhzz/url_0/url_463/193039199_mp4_h264_aac_hd_7.ts
#EXTINF:10.0,
https://test-streams.mux.dev/x36xhzz/url_0/url_464/193039199_mp4_h264_aac_hd_7.ts
#EXTINF:10.0,
https://test-streams.mux.dev/x36xhzz/url_0/url_465/193039199_mp4_h264_aac_hd_7.ts
#EXTINF:10.0,
https://test-streams.mux.dev/x36xhzz/url_0/url_466/193039199_mp4_h264_aac_hd_7.ts

#EXT-X-CUE-OUT:30
#EXTINF:10.0,
https://test-streams.mux.dev/x36xhzz/url_0/url_467/193039199_mp4_h264_aac_hd_7.ts
#EXT-X-CUE-OUT-CONT:10/30
#EXTINF:10.0,
https://test-streams.mux.dev/x36xhzz/url_0/url_468/193039199_mp4_h264_aac_hd_7.ts
#EXT-X-CUE-OUT-CONT:20/30
#EXTINF:10.0,
https://test-streams.mux.dev/x36xhzz/url_0/url_469/193039199_mp4_h264_aac_hd_7.ts
#EXT-X-CUE-IN

#EXTINF:10.0,
https://test-streams.mux.dev/x36xhzz/url_0/url_470/193039199_mp4_h264_aac_hd_7.ts
#EXTINF:10.0,
https://test-streams.mux.dev/x36xhzz/url_0/url_471/193039199_mp4_h264_aac_hd_7.ts
#EXTINF:10.0,
https://test-streams.mux.dev/x36xhzz/url_0/url_472/193039199_mp4_h264_aac_hd_7.ts

#EXT-X-ENDLIST
"#;

    info!("Demo playlist: 11 segments, 1 ad break (30s) at segments 5-7");

    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/vnd.apple.mpegurl")],
        playlist,
    )
        .into_response()
}

/// Demo DASH manifest endpoint for testing the DASH ad insertion pipeline
///
/// Serves a synthetic DASH MPD with SCTE-35 EventStream signaling to simulate
/// ad break opportunities. Uses Mux public test stream segments for content.
///
/// Usage:
///   1. Start Ritcher: `DEV_MODE=true cargo run`
///   2. Open a DASH player (dash.js, shaka-player, VLC)
///   3. Point to: http://localhost:3000/stitch/demo/manifest.mpd?origin=http://localhost:3000/demo/manifest.mpd
///
/// The stitcher will fetch this demo MPD, detect the EventStream signal,
/// and insert ad Periods with ad content.
pub async fn serve_demo_manifest() -> Response {
    info!("Serving demo DASH manifest with SCTE-35 EventStream");

    // Build a synthetic DASH MPD with real Mux test stream segments
    // and SCTE-35 EventStream signaling in the first period.
    //
    // Structure:
    // - Period 1 (60s): Content with EventStream indicating 30s ad break at 50s
    // - Period 2 (30s): More content
    //
    // The stitcher will detect the EventStream signal and insert an ad Period
    // between the two content periods.
    let manifest = r#"<?xml version="1.0" encoding="UTF-8"?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT90S" minBufferTime="PT2S" profiles="urn:mpeg:dash:profile:isoff-live:2011">
  <Period id="content-1" duration="PT60S">
    <BaseURL>https://test-streams.mux.dev/x36xhzz/url_0/</BaseURL>
    <AdaptationSet id="1" contentType="video" mimeType="video/mp2t">
      <Representation id="video" bandwidth="800000" codecs="avc1.64001f">
        <SegmentTemplate media="url_$Number$/193039199_mp4_h264_aac_hd_7.ts" timescale="1" duration="10" startNumber="462"/>
      </Representation>
    </AdaptationSet>
    <AdaptationSet id="2" contentType="audio" mimeType="audio/mp4" lang="en">
      <Representation id="audio" bandwidth="128000" codecs="mp4a.40.2">
        <SegmentTemplate media="url_$Number$/193039199_mp4_h264_aac_hd_7.ts" timescale="1" duration="10" startNumber="462"/>
      </Representation>
    </AdaptationSet>
    <EventStream schemeIdUri="urn:scte:scte35:2013:xml" timescale="1">
      <Event presentationTime="50" duration="30" id="ad-1">
        <scte35:SpliceInfoSection xmlns:scte35="http://www.scte.org/schemas/35/2016">
          <scte35:SpliceInsert spliceEventId="100" outOfNetworkIndicator="true">
            <scte35:BreakDuration autoReturn="true" duration="30"/>
          </scte35:SpliceInsert>
        </scte35:SpliceInfoSection>
      </Event>
    </EventStream>
  </Period>
  <Period id="content-2" duration="PT30S">
    <BaseURL>https://test-streams.mux.dev/x36xhzz/url_0/</BaseURL>
    <AdaptationSet id="1" contentType="video" mimeType="video/mp2t">
      <Representation id="video" bandwidth="800000" codecs="avc1.64001f">
        <SegmentTemplate media="url_$Number$/193039199_mp4_h264_aac_hd_7.ts" timescale="1" duration="10" startNumber="468"/>
      </Representation>
    </AdaptationSet>
    <AdaptationSet id="2" contentType="audio" mimeType="audio/mp4" lang="en">
      <Representation id="audio" bandwidth="128000" codecs="mp4a.40.2">
        <SegmentTemplate media="url_$Number$/193039199_mp4_h264_aac_hd_7.ts" timescale="1" duration="10" startNumber="468"/>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"#;

    info!("Demo manifest: 2 content periods (video+audio), 1 SCTE-35 signal (30s ad break at 50s)");

    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/dash+xml")],
        manifest,
    )
        .into_response()
}
