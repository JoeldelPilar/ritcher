use crate::error::{Result, RitcherError};
use quick_xml::events::Event;
use quick_xml::reader::Reader;

use super::types::MediaFile;

/// Get attribute value from an XML element
pub(crate) fn get_attr(e: &quick_xml::events::BytesStart, name: &str) -> Option<String> {
    e.attributes()
        .filter_map(|a| a.ok())
        .find(|a| a.key.as_ref() == name.as_bytes())
        .and_then(|a| String::from_utf8(a.value.to_vec()).ok())
}

/// Read text content from current element, handling CDATA
pub(crate) fn read_text(reader: &mut Reader<&[u8]>, end_tag: &str) -> Result<String> {
    let mut text = String::new();
    let end_tag_bytes = end_tag.as_bytes();

    loop {
        match reader.read_event() {
            Ok(Event::Text(e)) => {
                text.push_str(&e.unescape().unwrap_or_default());
            }
            Ok(Event::CData(e)) => {
                text.push_str(std::str::from_utf8(&e).unwrap_or_default());
            }
            Ok(Event::End(ref e)) if e.name().as_ref() == end_tag_bytes => break,
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(RitcherError::InternalError(format!(
                    "VAST XML read error: {}",
                    e
                )));
            }
            _ => {}
        }
    }

    Ok(text.trim().to_string())
}

/// Select the best media file for SSAI stitching
///
/// Prefers HLS streaming files (application/x-mpegURL) for segment-level
/// stitching, falls back to progressive MP4 if no streaming option available.
pub fn select_best_media_file(media_files: &[MediaFile]) -> Option<&MediaFile> {
    // Prefer HLS streaming for segment-level ad insertion
    let hls = media_files
        .iter()
        .find(|f| f.mime_type == "application/x-mpegURL");
    if hls.is_some() {
        return hls;
    }

    // Fallback: progressive MP4 with highest bitrate
    let mut progressive: Vec<&MediaFile> = media_files
        .iter()
        .filter(|f| f.delivery == "progressive" && f.mime_type == "video/mp4")
        .collect();
    progressive.sort_by(|a, b| b.bitrate.cmp(&a.bitrate));
    progressive.first().copied()
}

/// Parse VAST duration format "HH:MM:SS" or "HH:MM:SS.mmm" to seconds
pub(crate) fn parse_duration(duration: &str) -> f32 {
    let parts: Vec<&str> = duration.trim().split(':').collect();
    match parts.len() {
        3 => {
            let hours: f32 = parts[0].parse().unwrap_or(0.0);
            let minutes: f32 = parts[1].parse().unwrap_or(0.0);
            let seconds: f32 = parts[2].parse().unwrap_or(0.0);
            hours * 3600.0 + minutes * 60.0 + seconds
        }
        _ => {
            tracing::warn!("Invalid VAST duration format: {}", duration);
            0.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_duration() {
        assert_eq!(parse_duration("00:00:15"), 15.0);
        assert_eq!(parse_duration("00:00:30"), 30.0);
        assert_eq!(parse_duration("00:01:00"), 60.0);
        assert_eq!(parse_duration("01:00:00"), 3600.0);
        assert_eq!(parse_duration("00:00:10.5"), 10.5);
    }

    #[test]
    fn invalid_duration_formats() {
        assert_eq!(parse_duration(""), 0.0);
        assert_eq!(parse_duration("garbage"), 0.0);
        assert_eq!(parse_duration("00:00"), 0.0);
        assert_eq!(parse_duration("00:00:00:00"), 0.0);
        assert_eq!(parse_duration("::"), 0.0);
        assert_eq!(parse_duration("00:00:-5"), -5.0); // Negative parses but is unusual
    }

    #[test]
    fn test_select_best_media_file_prefers_hls() {
        let files = vec![
            MediaFile {
                url: "https://example.com/ad.mp4".to_string(),
                delivery: "progressive".to_string(),
                mime_type: "video/mp4".to_string(),
                width: 1280,
                height: 720,
                bitrate: Some(2000),
                codec: Some("H.264".to_string()),
            },
            MediaFile {
                url: "https://example.com/ad.m3u8".to_string(),
                delivery: "streaming".to_string(),
                mime_type: "application/x-mpegURL".to_string(),
                width: 1280,
                height: 720,
                bitrate: None,
                codec: None,
            },
        ];

        let best = select_best_media_file(&files).unwrap();
        assert_eq!(best.url, "https://example.com/ad.m3u8");
    }

    #[test]
    fn test_select_best_media_file_fallback_mp4() {
        let files = vec![MediaFile {
            url: "https://example.com/ad.mp4".to_string(),
            delivery: "progressive".to_string(),
            mime_type: "video/mp4".to_string(),
            width: 1280,
            height: 720,
            bitrate: Some(2000),
            codec: Some("H.264".to_string()),
        }];

        let best = select_best_media_file(&files).unwrap();
        assert_eq!(best.url, "https://example.com/ad.mp4");
    }

    #[test]
    fn select_best_media_file_empty_list() {
        let files: Vec<MediaFile> = vec![];
        assert!(
            select_best_media_file(&files).is_none(),
            "Empty media files should return None"
        );
    }

    #[test]
    fn select_best_media_file_highest_bitrate_fallback() {
        let files = vec![
            MediaFile {
                url: "https://example.com/low.mp4".to_string(),
                delivery: "progressive".to_string(),
                mime_type: "video/mp4".to_string(),
                width: 640,
                height: 360,
                bitrate: Some(500),
                codec: None,
            },
            MediaFile {
                url: "https://example.com/high.mp4".to_string(),
                delivery: "progressive".to_string(),
                mime_type: "video/mp4".to_string(),
                width: 1920,
                height: 1080,
                bitrate: Some(5000),
                codec: None,
            },
        ];
        let best = select_best_media_file(&files).unwrap();
        assert_eq!(
            best.url, "https://example.com/high.mp4",
            "Should pick highest bitrate MP4"
        );
    }
}
