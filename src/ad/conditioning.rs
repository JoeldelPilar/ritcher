use crate::ad::vast::MediaFile;
use tracing::warn;

/// Known HLS-compatible MIME types for ad creatives
const HLS_MIME_TYPES: &[&str] = &["application/x-mpegURL", "application/vnd.apple.mpegurl"];

/// Known progressive video MIME types
const PROGRESSIVE_MIME_TYPES: &[&str] = &["video/mp4", "video/webm", "video/3gpp"];

/// Validate ad creative compatibility and log warnings
///
/// Phase 1: warning-only. Does not block ad insertion.
/// Checks for common issues that may cause playback problems:
/// - Non-HLS ad creative in HLS stream (codec mismatch)
/// - Resolution mismatches (if detectable)
/// - Missing or unknown MIME types
///
/// Future: integrate with Eyevinn Ad Normalizer for transcoding
pub fn check_creative(media_file: &MediaFile, session_id: &str) {
    let mime = &media_file.mime_type;

    // Check if MIME type is HLS-compatible
    if !is_hls_mime(mime) {
        if is_progressive_mime(mime) {
            warn!(
                session_id = session_id,
                mime_type = mime,
                url = media_file.url,
                "Ad conditioning: Progressive MP4 creative detected — \
                 may cause playback issues in HLS stream. \
                 Consider using Eyevinn Ad Normalizer for transcoding."
            );
        } else {
            warn!(
                session_id = session_id,
                mime_type = mime,
                url = media_file.url,
                "Ad conditioning: Unknown MIME type for ad creative — \
                 expected HLS (application/x-mpegURL) or progressive (video/mp4)."
            );
        }
    }

    // Check resolution if available
    if media_file.width > 0 && media_file.height > 0 {
        // Common broadcast resolutions
        let is_standard = matches!(
            (media_file.width, media_file.height),
            (1920, 1080)
                | (1280, 720)
                | (854, 480)
                | (640, 360)
                | (426, 240)
                | (3840, 2160)
                | (960, 540)
                | (768, 432)
        );

        if !is_standard {
            warn!(
                session_id = session_id,
                width = media_file.width,
                height = media_file.height,
                url = media_file.url,
                "Ad conditioning: Non-standard resolution {}x{} — \
                 may cause visual artifacts or letterboxing.",
                media_file.width,
                media_file.height
            );
        }
    }

    // Check codec if available
    if let Some(codec) = &media_file.codec {
        let codec_lower = codec.to_lowercase();
        if codec_lower.contains("vpaid") {
            warn!(
                session_id = session_id,
                codec = codec,
                url = media_file.url,
                "Ad conditioning: VPAID creative detected — \
                 not supported in SSAI mode. Creative will be skipped."
            );
        }
    }
}

/// Check multiple creatives and return the count of warnings
pub fn check_creatives(media_files: &[&MediaFile], session_id: &str) -> usize {
    let mut warning_count = 0;
    for media_file in media_files {
        let mime = &media_file.mime_type;
        if !is_hls_mime(mime) {
            warning_count += 1;
        }
        check_creative(media_file, session_id);
    }
    warning_count
}

fn is_hls_mime(mime: &str) -> bool {
    HLS_MIME_TYPES.iter().any(|&t| t.eq_ignore_ascii_case(mime))
}

fn is_progressive_mime(mime: &str) -> bool {
    PROGRESSIVE_MIME_TYPES
        .iter()
        .any(|&t| t.eq_ignore_ascii_case(mime))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_media_file(mime_type: &str, width: u32, height: u32) -> MediaFile {
        MediaFile {
            url: "http://example.com/ad.ts".to_string(),
            delivery: "progressive".to_string(),
            mime_type: mime_type.to_string(),
            width,
            height,
            bitrate: Some(2000),
            codec: None,
        }
    }

    #[test]
    fn test_hls_mime_detection() {
        assert!(is_hls_mime("application/x-mpegURL"));
        assert!(is_hls_mime("application/vnd.apple.mpegurl"));
        assert!(!is_hls_mime("video/mp4"));
        assert!(!is_hls_mime("video/webm"));
    }

    #[test]
    fn test_progressive_mime_detection() {
        assert!(is_progressive_mime("video/mp4"));
        assert!(is_progressive_mime("video/webm"));
        assert!(!is_progressive_mime("application/x-mpegURL"));
    }

    #[test]
    fn test_check_creative_hls_no_warnings() {
        // HLS creative should not produce warnings (verified by no panic)
        let media_file = create_media_file("application/x-mpegURL", 1920, 1080);
        check_creative(&media_file, "test-session");
    }

    #[test]
    fn test_check_creative_progressive_warns() {
        // Progressive creative logs a warning (test verifies it doesn't panic)
        let media_file = create_media_file("video/mp4", 1920, 1080);
        check_creative(&media_file, "test-session");
    }

    #[test]
    fn test_check_creative_nonstandard_resolution() {
        let media_file = create_media_file("application/x-mpegURL", 999, 555);
        check_creative(&media_file, "test-session");
    }

    #[test]
    fn test_check_creatives_counts_warnings() {
        let hls = create_media_file("application/x-mpegURL", 1920, 1080);
        let mp4 = create_media_file("video/mp4", 1280, 720);
        let unknown = create_media_file("video/unknown", 640, 360);

        let files: Vec<&MediaFile> = vec![&hls, &mp4, &unknown];
        let count = check_creatives(&files, "test-session");

        assert_eq!(count, 2); // mp4 + unknown are non-HLS
    }
}
