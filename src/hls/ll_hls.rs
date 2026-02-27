//! Low-Latency HLS (LL-HLS) tag pass-through for SGAI stitching
//!
//! m3u8-rs 6.0 drops playlist-level unknown tags during parsing, which means
//! LL-HLS-specific tags (`EXT-X-SERVER-CONTROL`, `EXT-X-PART-INF`,
//! `EXT-X-SKIP`, `EXT-X-PART`, `EXT-X-PRELOAD-HINT`, `EXT-X-RENDITION-REPORT`)
//! are lost after a parse-serialize round-trip.
//!
//! This module provides a hybrid approach:
//! 1. **Extract** LL-HLS playlist-level tags from raw content before parsing
//! 2. **Re-inject** them into the serialized output after m3u8-rs serialization
//! 3. **Rewrite** URIs in line-level tags (PART, PRELOAD-HINT, RENDITION-REPORT)
//!    to route through the stitcher's proxy endpoints

use tracing::debug;

/// Playlist-level LL-HLS tags that m3u8-rs drops during parsing.
///
/// Each field stores the complete raw line (including the `#EXT-X-` prefix)
/// so it can be re-injected verbatim into the serialized output.
#[derive(Debug, Clone, Default)]
pub struct LlHlsPlaylistTags {
    pub server_control: Option<String>,
    pub part_inf: Option<String>,
    pub skip: Option<String>,
    /// `EXT-X-PRELOAD-HINT` lines — appear after the last segment and are
    /// dropped by m3u8-rs because there is no segment to attach them to.
    pub preload_hints: Vec<String>,
    /// `EXT-X-RENDITION-REPORT` lines — one per alternative rendition,
    /// appear at the end of the playlist and are also dropped by m3u8-rs.
    pub rendition_reports: Vec<String>,
}

/// Cheap check for whether the playlist content is LL-HLS.
///
/// Returns `true` if the content contains any of the LL-HLS indicator tags.
/// This is used to gate the more expensive extract/inject/rewrite code path.
pub fn is_ll_hls(content: &str) -> bool {
    content.contains("#EXT-X-SERVER-CONTROL:")
        || content.contains("#EXT-X-PART-INF:")
        || content.contains("#EXT-X-PART:")
}

/// Scan raw playlist content and capture LL-HLS playlist-level tags.
///
/// Extracts the full raw line for `EXT-X-SERVER-CONTROL`, `EXT-X-PART-INF`,
/// and `EXT-X-SKIP`. These tags are stored verbatim so they can be re-injected
/// after m3u8-rs serialization without any attribute loss.
pub fn extract_ll_hls_tags(content: &str) -> LlHlsPlaylistTags {
    let mut tags = LlHlsPlaylistTags::default();

    for line in content.lines() {
        if line.starts_with("#EXT-X-SERVER-CONTROL:") {
            debug!("LL-HLS: captured SERVER-CONTROL tag");
            tags.server_control = Some(line.to_string());
        } else if line.starts_with("#EXT-X-PART-INF:") {
            debug!("LL-HLS: captured PART-INF tag");
            tags.part_inf = Some(line.to_string());
        } else if line.starts_with("#EXT-X-SKIP:") {
            debug!("LL-HLS: captured SKIP tag");
            tags.skip = Some(line.to_string());
        } else if line.starts_with("#EXT-X-PRELOAD-HINT:") {
            debug!("LL-HLS: captured PRELOAD-HINT tag");
            tags.preload_hints.push(line.to_string());
        } else if line.starts_with("#EXT-X-RENDITION-REPORT:") {
            debug!("LL-HLS: captured RENDITION-REPORT tag");
            tags.rendition_reports.push(line.to_string());
        }
    }

    tags
}

/// Re-inject captured LL-HLS tags into the serialized playlist output.
///
/// Tags are inserted after the `#EXT-X-TARGETDURATION:` line (the natural
/// position per the HLS spec). Falls back to after `#EXT-X-VERSION:` or
/// `#EXTM3U` if TARGETDURATION is not present.
///
/// Injection order: SERVER-CONTROL, PART-INF, SKIP (matches typical encoder
/// output and spec examples).
///
/// If all tags are `None`, the input is returned unchanged with no allocation.
pub fn inject_ll_hls_tags(serialized: &str, tags: &LlHlsPlaylistTags) -> String {
    let has_header_tags =
        tags.server_control.is_some() || tags.part_inf.is_some() || tags.skip.is_some();
    let has_tail_tags = !tags.preload_hints.is_empty() || !tags.rendition_reports.is_empty();

    if !has_header_tags && !has_tail_tags {
        return serialized.to_string();
    }

    let mut result = String::with_capacity(serialized.len() + 512);

    if has_header_tags {
        // Find the insertion point: after TARGETDURATION, VERSION, or EXTM3U
        let insertion_line = find_insertion_line(serialized);

        for (idx, line) in serialized.lines().enumerate() {
            result.push_str(line);
            result.push('\n');

            if idx == insertion_line {
                if let Some(ref sc) = tags.server_control {
                    result.push_str(sc);
                    result.push('\n');
                }
                if let Some(ref pi) = tags.part_inf {
                    result.push_str(pi);
                    result.push('\n');
                }
                if let Some(ref sk) = tags.skip {
                    result.push_str(sk);
                    result.push('\n');
                }
            }
        }
    } else {
        result.push_str(serialized);
        // Ensure trailing newline before appending tail tags
        if !result.ends_with('\n') {
            result.push('\n');
        }
    }

    // Append tail tags at the end of the playlist
    // (PRELOAD-HINT and RENDITION-REPORT appear after the last segment)
    for hint in &tags.preload_hints {
        result.push_str(hint);
        result.push('\n');
    }
    for report in &tags.rendition_reports {
        result.push_str(report);
        result.push('\n');
    }

    result
}

/// Find the zero-based line index after which LL-HLS tags should be injected.
///
/// Priority: TARGETDURATION > VERSION > EXTM3U (line 0).
fn find_insertion_line(content: &str) -> usize {
    let mut target_duration_line = None;
    let mut version_line = None;
    let mut extm3u_line = None;

    for (idx, line) in content.lines().enumerate() {
        if line.starts_with("#EXT-X-TARGETDURATION:") {
            target_duration_line = Some(idx);
            // TARGETDURATION is the preferred anchor; stop searching
            break;
        } else if line.starts_with("#EXT-X-VERSION:") {
            version_line = Some(idx);
        } else if line.starts_with("#EXTM3U") {
            extm3u_line = Some(idx);
        }
    }

    target_duration_line
        .or(version_line)
        .or(extm3u_line)
        .unwrap_or(0)
}

/// Rewrite URIs in LL-HLS line-level tags to route through the stitcher.
///
/// Processes each line and rewrites URIs in:
/// - `#EXT-X-PART:` — segment proxy (`/stitch/{id}/segment/{name}`)
/// - `#EXT-X-PRELOAD-HINT:` — segment proxy
/// - `#EXT-X-RENDITION-REPORT:` — playlist proxy (`/stitch/{id}/playlist.m3u8`)
///
/// Both relative and absolute URIs are handled. Relative URIs are resolved
/// against `origin_base`; absolute URIs have their origin extracted from the
/// URL itself.
pub fn rewrite_ll_hls_uris(
    serialized: &str,
    session_id: &str,
    base_url: &str,
    origin_base: &str,
) -> String {
    let mut result = String::with_capacity(serialized.len() + 512);

    for line in serialized.lines() {
        if line.starts_with("#EXT-X-PART:") || line.starts_with("#EXT-X-PRELOAD-HINT:") {
            result.push_str(&rewrite_segment_uri(
                line,
                session_id,
                base_url,
                origin_base,
            ));
        } else if line.starts_with("#EXT-X-RENDITION-REPORT:") {
            result.push_str(&rewrite_playlist_uri(
                line,
                session_id,
                base_url,
                origin_base,
            ));
        } else {
            result.push_str(line);
        }
        result.push('\n');
    }

    result
}

/// Extract the quoted URI value from a tag line.
///
/// Searches for `URI="` in the line and returns:
/// - The URI value (without quotes)
/// - The byte offset of the opening quote
/// - The byte offset one past the closing quote
///
/// Returns `None` if no `URI="..."` is found.
pub fn extract_quoted_uri(line: &str) -> Option<(String, usize, usize)> {
    let uri_marker = "URI=\"";
    let marker_pos = line.find(uri_marker)?;
    let value_start = marker_pos + uri_marker.len();
    let rest = &line[value_start..];
    let closing_quote = rest.find('"')?;
    let value = rest[..closing_quote].to_string();

    // quote_start is the position of the opening quote character
    let quote_start = value_start - 1;
    // quote_end is one past the closing quote character
    let quote_end = value_start + closing_quote + 1;

    Some((value, quote_start, quote_end))
}

/// Rewrite the URI in a PART or PRELOAD-HINT tag to the segment proxy.
fn rewrite_segment_uri(line: &str, session_id: &str, base_url: &str, origin_base: &str) -> String {
    let (uri_value, quote_start, quote_end) = match extract_quoted_uri(line) {
        Some(v) => v,
        None => return line.to_string(),
    };

    let (segment_name, origin) =
        if uri_value.starts_with("http://") || uri_value.starts_with("https://") {
            // Absolute URI: split into origin + segment name
            match uri_value.rsplit_once('/') {
                Some((base, name)) => (name.to_string(), base.to_string()),
                None => (uri_value.clone(), origin_base.to_string()),
            }
        } else {
            // Relative URI: use origin_base
            (uri_value.clone(), origin_base.to_string())
        };

    let new_uri = format!(
        "\"{}/stitch/{}/segment/{}?origin={}\"",
        base_url, session_id, segment_name, origin
    );

    let mut result = String::with_capacity(line.len() + new_uri.len());
    result.push_str(&line[..quote_start]);
    result.push_str(&new_uri);
    result.push_str(&line[quote_end..]);
    result
}

/// Rewrite the URI in a RENDITION-REPORT tag to the playlist proxy.
fn rewrite_playlist_uri(line: &str, session_id: &str, base_url: &str, origin_base: &str) -> String {
    let (uri_value, quote_start, quote_end) = match extract_quoted_uri(line) {
        Some(v) => v,
        None => return line.to_string(),
    };

    let absolute_url = if uri_value.starts_with("http://") || uri_value.starts_with("https://") {
        uri_value.clone()
    } else {
        format!("{}/{}", origin_base, uri_value)
    };

    let new_uri = format!(
        "\"{}/stitch/{}/playlist.m3u8?origin={}\"",
        base_url, session_id, absolute_url
    );

    let mut result = String::with_capacity(line.len() + new_uri.len());
    result.push_str(&line[..quote_start]);
    result.push_str(&new_uri);
    result.push_str(&line[quote_end..]);
    result
}

// -- Tests -------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Sample LL-HLS media playlist used across tests
    const LL_HLS_PLAYLIST: &str = "\
#EXTM3U
#EXT-X-VERSION:6
#EXT-X-TARGETDURATION:4
#EXT-X-SERVER-CONTROL:CAN-BLOCK-RELOAD=YES,PART-HOLD-BACK=1.0,CAN-SKIP-UNTIL=12.0
#EXT-X-PART-INF:PART-TARGET=0.33334
#EXT-X-MEDIA-SEQUENCE:80
#EXT-X-PROGRAM-DATE-TIME:2026-01-01T00:00:00.000Z
#EXT-X-PART:DURATION=0.33334,URI=\"seg80.0.mp4\",INDEPENDENT=YES
#EXT-X-PART:DURATION=0.33334,URI=\"seg80.1.mp4\"
#EXT-X-PART:DURATION=0.33334,URI=\"seg80.2.mp4\"
#EXTINF:1.0,
seg80.mp4
#EXT-X-PRELOAD-HINT:TYPE=PART,URI=\"seg81.0.mp4\"
#EXT-X-RENDITION-REPORT:URI=\"720p.m3u8\",LAST-MSN=80,LAST-PART=2";

    /// Regular HLS playlist (no LL-HLS tags)
    const REGULAR_PLAYLIST: &str = "\
#EXTM3U
#EXT-X-VERSION:3
#EXT-X-TARGETDURATION:10
#EXTINF:10,
seg0.ts
#EXTINF:10,
seg1.ts
#EXT-X-ENDLIST";

    // -- is_ll_hls -----------------------------------------------------------

    #[test]
    fn test_is_ll_hls_with_server_control() {
        assert!(is_ll_hls(LL_HLS_PLAYLIST));
    }

    #[test]
    fn test_is_ll_hls_with_part_tag() {
        // Only EXT-X-PART, no SERVER-CONTROL or PART-INF
        let content = "#EXTM3U\n#EXT-X-PART:DURATION=0.5,URI=\"p.mp4\"";
        assert!(is_ll_hls(content));
    }

    #[test]
    fn test_is_ll_hls_regular_hls() {
        assert!(!is_ll_hls(REGULAR_PLAYLIST));
    }

    // -- extract_ll_hls_tags -------------------------------------------------

    #[test]
    fn test_extract_all_tags() {
        let tags = extract_ll_hls_tags(LL_HLS_PLAYLIST);

        assert!(tags.server_control.is_some());
        assert!(tags.part_inf.is_some());
        assert!(tags.skip.is_none()); // sample has no SKIP

        assert_eq!(
            tags.server_control.unwrap(),
            "#EXT-X-SERVER-CONTROL:CAN-BLOCK-RELOAD=YES,PART-HOLD-BACK=1.0,CAN-SKIP-UNTIL=12.0"
        );
        assert_eq!(
            tags.part_inf.unwrap(),
            "#EXT-X-PART-INF:PART-TARGET=0.33334"
        );

        // PRELOAD-HINT and RENDITION-REPORT should also be captured
        assert_eq!(tags.preload_hints.len(), 1);
        assert!(tags.preload_hints[0].starts_with("#EXT-X-PRELOAD-HINT:"));
        assert_eq!(tags.rendition_reports.len(), 1);
        assert!(tags.rendition_reports[0].starts_with("#EXT-X-RENDITION-REPORT:"));
    }

    #[test]
    fn test_extract_with_skip() {
        let content = "\
#EXTM3U
#EXT-X-VERSION:6
#EXT-X-TARGETDURATION:4
#EXT-X-SERVER-CONTROL:CAN-BLOCK-RELOAD=YES,PART-HOLD-BACK=1.0
#EXT-X-PART-INF:PART-TARGET=0.5
#EXT-X-SKIP:SKIPPED-SEGMENTS=3
#EXTINF:2.0,
seg10.ts";

        let tags = extract_ll_hls_tags(content);

        assert!(tags.skip.is_some());
        assert_eq!(tags.skip.unwrap(), "#EXT-X-SKIP:SKIPPED-SEGMENTS=3");
    }

    #[test]
    fn test_extract_preserves_full_line() {
        let raw_line =
            "#EXT-X-SERVER-CONTROL:CAN-BLOCK-RELOAD=YES,PART-HOLD-BACK=1.0,CAN-SKIP-UNTIL=12.0";
        let content = format!("#EXTM3U\n{}\n#EXTINF:2.0,\nseg.ts", raw_line);

        let tags = extract_ll_hls_tags(&content);
        assert_eq!(tags.server_control.as_deref(), Some(raw_line));
    }

    // -- inject_ll_hls_tags --------------------------------------------------

    #[test]
    fn test_inject_after_targetduration() {
        let serialized = "\
#EXTM3U
#EXT-X-VERSION:6
#EXT-X-TARGETDURATION:4
#EXT-X-MEDIA-SEQUENCE:80
#EXTINF:1.0,
seg80.mp4
";

        let tags = LlHlsPlaylistTags {
            server_control: Some(
                "#EXT-X-SERVER-CONTROL:CAN-BLOCK-RELOAD=YES,PART-HOLD-BACK=1.0".to_string(),
            ),
            part_inf: Some("#EXT-X-PART-INF:PART-TARGET=0.33334".to_string()),
            skip: None,
            preload_hints: vec![],
            rendition_reports: vec![],
        };

        let result = inject_ll_hls_tags(serialized, &tags);

        // Tags should appear after TARGETDURATION and before MEDIA-SEQUENCE
        let lines: Vec<&str> = result.lines().collect();
        let td_pos = lines
            .iter()
            .position(|l| l.starts_with("#EXT-X-TARGETDURATION:"))
            .unwrap();
        let sc_pos = lines
            .iter()
            .position(|l| l.starts_with("#EXT-X-SERVER-CONTROL:"))
            .unwrap();
        let pi_pos = lines
            .iter()
            .position(|l| l.starts_with("#EXT-X-PART-INF:"))
            .unwrap();
        let ms_pos = lines
            .iter()
            .position(|l| l.starts_with("#EXT-X-MEDIA-SEQUENCE:"))
            .unwrap();

        assert!(
            sc_pos > td_pos,
            "SERVER-CONTROL should be after TARGETDURATION"
        );
        assert!(pi_pos > sc_pos, "PART-INF should be after SERVER-CONTROL");
        assert!(
            ms_pos > pi_pos,
            "MEDIA-SEQUENCE should be after injected tags"
        );
    }

    #[test]
    fn test_inject_empty_tags_noop() {
        let serialized = "#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXTINF:1.0,\nseg.ts\n";
        let tags = LlHlsPlaylistTags::default();

        let result = inject_ll_hls_tags(serialized, &tags);
        assert_eq!(result, serialized);
    }

    // -- extract_quoted_uri --------------------------------------------------

    #[test]
    fn test_extract_quoted_uri_basic() {
        let line = "#EXT-X-PART:DURATION=0.33334,URI=\"seg80.0.mp4\",INDEPENDENT=YES";
        let (value, start, end) = extract_quoted_uri(line).unwrap();

        assert_eq!(value, "seg80.0.mp4");
        // The quoted section URI="seg80.0.mp4" should be captured
        assert_eq!(&line[start..end], "\"seg80.0.mp4\"");
    }

    #[test]
    fn test_extract_quoted_uri_with_attributes_after() {
        let line = "#EXT-X-RENDITION-REPORT:URI=\"720p.m3u8\",LAST-MSN=80,LAST-PART=2";
        let (value, start, end) = extract_quoted_uri(line).unwrap();

        assert_eq!(value, "720p.m3u8");
        assert_eq!(&line[start..end], "\"720p.m3u8\"");
        // Text after closing quote should be intact
        assert_eq!(&line[end..], ",LAST-MSN=80,LAST-PART=2");
    }

    // -- rewrite_ll_hls_uris -------------------------------------------------

    #[test]
    fn test_rewrite_part_uris() {
        let input = "#EXT-X-PART:DURATION=0.33334,URI=\"seg80.0.mp4\",INDEPENDENT=YES\n";

        let result = rewrite_ll_hls_uris(
            input,
            "sess-1",
            "http://stitch.test",
            "http://cdn.test/live",
        );

        assert!(
            result.contains(
                "URI=\"http://stitch.test/stitch/sess-1/segment/seg80.0.mp4?origin=http://cdn.test/live\""
            ),
            "Rewritten PART URI not found in: {}",
            result
        );
        // Other attributes preserved
        assert!(result.contains("DURATION=0.33334"));
        assert!(result.contains("INDEPENDENT=YES"));
    }

    #[test]
    fn test_rewrite_preload_hint() {
        let input = "#EXT-X-PRELOAD-HINT:TYPE=PART,URI=\"seg81.0.mp4\"\n";

        let result = rewrite_ll_hls_uris(
            input,
            "sess-1",
            "http://stitch.test",
            "http://cdn.test/live",
        );

        assert!(
            result.contains(
                "URI=\"http://stitch.test/stitch/sess-1/segment/seg81.0.mp4?origin=http://cdn.test/live\""
            ),
            "Rewritten PRELOAD-HINT URI not found in: {}",
            result
        );
        assert!(result.contains("TYPE=PART"));
    }

    #[test]
    fn test_rewrite_rendition_report() {
        let input = "#EXT-X-RENDITION-REPORT:URI=\"720p.m3u8\",LAST-MSN=80,LAST-PART=2\n";

        let result = rewrite_ll_hls_uris(
            input,
            "sess-1",
            "http://stitch.test",
            "http://cdn.test/live",
        );

        assert!(
            result.contains(
                "URI=\"http://stitch.test/stitch/sess-1/playlist.m3u8?origin=http://cdn.test/live/720p.m3u8\""
            ),
            "Rewritten RENDITION-REPORT URI not found in: {}",
            result
        );
        assert!(result.contains("LAST-MSN=80"));
        assert!(result.contains("LAST-PART=2"));
    }

    #[test]
    fn test_rewrite_absolute_uri() {
        let input = "#EXT-X-PART:DURATION=0.5,URI=\"http://cdn.test/live/seg80.0.mp4\"\n";

        let result =
            rewrite_ll_hls_uris(input, "sess-1", "http://stitch.test", "http://other.test");

        // Origin should be extracted from the absolute URL, not from origin_base
        assert!(
            result.contains(
                "URI=\"http://stitch.test/stitch/sess-1/segment/seg80.0.mp4?origin=http://cdn.test/live\""
            ),
            "Absolute URI origin not extracted correctly in: {}",
            result
        );
    }

    #[test]
    fn test_passthrough_non_ll_hls_lines() {
        let input = "\
#EXTM3U
#EXT-X-VERSION:6
#EXT-X-TARGETDURATION:4
#EXTINF:1.0,
seg80.mp4
";

        let result = rewrite_ll_hls_uris(
            input,
            "sess-1",
            "http://stitch.test",
            "http://cdn.test/live",
        );

        // Non-LL-HLS lines should pass through unchanged
        assert!(result.contains("#EXTM3U"));
        assert!(result.contains("#EXT-X-VERSION:6"));
        assert!(result.contains("#EXT-X-TARGETDURATION:4"));
        assert!(result.contains("#EXTINF:1.0,"));
        assert!(result.contains("seg80.mp4"));
    }

    #[test]
    fn test_full_roundtrip() {
        // Simulate the full pipeline: extract tags, (parse+serialize drops them),
        // then inject tags back, then rewrite URIs.

        // 1. Extract tags from original content
        let tags = extract_ll_hls_tags(LL_HLS_PLAYLIST);
        assert!(tags.server_control.is_some());
        assert!(tags.part_inf.is_some());

        // 2. Simulate m3u8-rs serialization output (tags dropped, PARTs dropped)
        let serialized_by_m3u8rs = "\
#EXTM3U
#EXT-X-VERSION:6
#EXT-X-TARGETDURATION:4
#EXT-X-MEDIA-SEQUENCE:80
#EXT-X-PROGRAM-DATE-TIME:2026-01-01T00:00:00.000Z
#EXTINF:1.0,
seg80.mp4
";

        // 3. Re-inject playlist-level tags
        let with_tags = inject_ll_hls_tags(serialized_by_m3u8rs, &tags);
        assert!(with_tags.contains("#EXT-X-SERVER-CONTROL:"));
        assert!(with_tags.contains("#EXT-X-PART-INF:"));

        // Verify injection order: SERVER-CONTROL before PART-INF
        let sc_pos = with_tags.find("#EXT-X-SERVER-CONTROL:").unwrap();
        let pi_pos = with_tags.find("#EXT-X-PART-INF:").unwrap();
        assert!(
            sc_pos < pi_pos,
            "SERVER-CONTROL should come before PART-INF"
        );

        // 4. Re-inject tail tags (PRELOAD-HINT, RENDITION-REPORT) at the end
        // These are also dropped by m3u8-rs since they appear after the last segment.
        // The inject function handles both header tags (after TARGETDURATION) and
        // tail tags (appended at the end).

        // Now add back the line-level segment tags (EXT-X-PART) which m3u8-rs preserved,
        // then the tail tags which were re-injected.
        let with_parts = format!(
            "{}\
#EXT-X-PART:DURATION=0.33334,URI=\"seg80.0.mp4\",INDEPENDENT=YES
#EXT-X-PART:DURATION=0.33334,URI=\"seg80.1.mp4\"
#EXT-X-PART:DURATION=0.33334,URI=\"seg80.2.mp4\"
",
            with_tags
        );
        // In the real pipeline, PRELOAD-HINT and RENDITION-REPORT are injected
        // by inject_ll_hls_tags. Let's verify they're already in with_tags.
        assert!(
            with_tags.contains("#EXT-X-PRELOAD-HINT:"),
            "PRELOAD-HINT should have been injected as tail tag"
        );
        assert!(
            with_tags.contains("#EXT-X-RENDITION-REPORT:"),
            "RENDITION-REPORT should have been injected as tail tag"
        );

        let final_output = rewrite_ll_hls_uris(
            &with_parts,
            "sess-42",
            "http://stitch.test",
            "http://cdn.test/live",
        );

        // Verify all LL-HLS tags present
        assert!(final_output.contains("#EXT-X-SERVER-CONTROL:"));
        assert!(final_output.contains("#EXT-X-PART-INF:"));

        // Verify all PARTs rewritten
        assert!(
            final_output
                .contains("/stitch/sess-42/segment/seg80.0.mp4?origin=http://cdn.test/live")
        );
        assert!(
            final_output
                .contains("/stitch/sess-42/segment/seg80.1.mp4?origin=http://cdn.test/live")
        );
        assert!(
            final_output
                .contains("/stitch/sess-42/segment/seg80.2.mp4?origin=http://cdn.test/live")
        );

        // Verify PRELOAD-HINT rewritten
        assert!(
            final_output
                .contains("/stitch/sess-42/segment/seg81.0.mp4?origin=http://cdn.test/live")
        );

        // Verify RENDITION-REPORT rewritten to playlist endpoint
        assert!(
            final_output
                .contains("/stitch/sess-42/playlist.m3u8?origin=http://cdn.test/live/720p.m3u8")
        );

        // Verify regular content segment NOT rewritten (that is parser.rs's job)
        assert!(final_output.contains("\nseg80.mp4\n"));
    }
}
