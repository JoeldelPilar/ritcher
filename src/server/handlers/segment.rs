use crate::{
    error::Result,
    http_retry::{RetryConfig, fetch_with_retry},
    metrics,
    server::{
        state::AppState,
        url_validation::{validate_origin_url, validate_session_id},
    },
};
use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use std::collections::HashMap;
use std::time::Instant;
use tracing::{info, warn};

/// Reject segment paths containing path traversal sequences.
///
/// Checks for `..` components that could escape the intended directory,
/// whether URL-encoded (`%2e%2e`, `%2E%2E`), double-encoded (`%252e%252e`),
/// or literal. Decodes in a loop until the string stabilizes so that
/// multi-layered encoding cannot bypass the check.
fn validate_segment_path(path: &str) -> Result<()> {
    let mut current = path.to_string();
    loop {
        let decoded = percent_decode(&current);
        if decoded.contains("..") {
            warn!("Path traversal attempt blocked: {}", path);
            return Err(crate::error::RitcherError::InvalidOrigin(
                "Invalid segment path".to_string(),
            ));
        }
        if decoded == current {
            break;
        }
        current = decoded;
    }
    Ok(())
}

/// Simple percent-decoding for path traversal detection.
///
/// Decodes `%XX` sequences so that encoded `..` patterns like `%2e%2e`
/// are caught by the traversal check.
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2]))
        {
            decoded.push(hi << 4 | lo);
            i += 3;
            continue;
        }
        decoded.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

/// Convert a single ASCII hex character to its numeric value.
fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Proxy content segments from origin to the player.
///
/// Validates the segment path against path-traversal attacks, then streams
/// the segment from the origin CDN to the client without buffering.
/// Uses [`fetch_with_retry`] for fault-tolerant HTTP fetching.
pub async fn serve_segment(
    Path((session_id, segment_path)): Path<(String, String)>,
    Query(params): Query<HashMap<String, String>>,
    State(state): State<AppState>,
) -> Result<Response> {
    validate_session_id(&session_id)?;
    let start = Instant::now();
    info!(
        "Serving segment: {} for session: {}",
        segment_path, session_id
    );

    // Block path traversal attempts (e.g. "../../etc/passwd")
    validate_segment_path(&segment_path)?;

    // Get origin base URL from query params or fallback to config.
    // Validate user-supplied origin against SSRF attack vectors.
    let origin_base: &str = if let Some(origin) = params.get("origin") {
        validate_origin_url(origin)?;
        origin.as_str()
    } else {
        &state.config.origin_url
    };

    let segment_url = format!("{}/{}", origin_base, segment_path);

    info!("Fetching segment from origin: {}", segment_url);

    match fetch_with_retry(&state.http_client, &segment_url, &RetryConfig::default()).await {
        Ok(response) => {
            let content_type = response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("video/MP2T")
                .to_string();

            metrics::record_request("segment", 200);
            metrics::record_duration("segment", start);

            Ok((
                StatusCode::OK,
                [(header::CONTENT_TYPE, content_type.as_str())],
                Body::from_stream(response.bytes_stream()),
            )
                .into_response())
        }
        Err(e) => {
            metrics::record_origin_error();
            metrics::record_request("segment", 502);
            metrics::record_duration("segment", start);

            Err(crate::error::RitcherError::OriginFetchError(e))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- validate_segment_path ---

    #[test]
    fn rejects_literal_dot_dot() {
        assert!(validate_segment_path("../../etc/passwd").is_err());
        assert!(validate_segment_path("foo/../bar.ts").is_err());
        assert!(validate_segment_path("../secret").is_err());
        assert!(validate_segment_path("..").is_err());
    }

    #[test]
    fn rejects_encoded_dot_dot() {
        // %2e = '.'
        assert!(validate_segment_path("%2e%2e/etc/passwd").is_err());
        assert!(validate_segment_path("%2E%2E/etc/passwd").is_err());
        assert!(validate_segment_path("foo/%2e%2e/bar.ts").is_err());
        assert!(validate_segment_path("foo/%2e./bar.ts").is_err());
        assert!(validate_segment_path("foo/.%2e/bar.ts").is_err());
    }

    #[test]
    fn rejects_single_percent_encoded_dot_dot() {
        // %2e%2e -> ".." after one decode pass
        assert!(validate_segment_path("foo/%2e%2e/bar").is_err());
    }

    #[test]
    fn rejects_double_percent_encoded_dot_dot() {
        // %252e%252e -> %2e%2e (1st decode) -> ".." (2nd decode)
        assert!(validate_segment_path("%252e%252e/etc/passwd").is_err());
        assert!(validate_segment_path("foo/%252e%252e/bar").is_err());
    }

    #[test]
    fn rejects_triple_percent_encoded_dot_dot() {
        // %25252e%25252e -> %252e%252e (1st) -> %2e%2e (2nd) -> ".." (3rd)
        assert!(validate_segment_path("%25252e%25252e/etc/passwd").is_err());
        assert!(validate_segment_path("foo/%25252e%25252e/bar").is_err());
    }

    #[test]
    fn allows_normal_segment_paths() {
        assert!(validate_segment_path("stream/720p/segment001.ts").is_ok());
        assert!(validate_segment_path("hls/live/seg-42.ts").is_ok());
        assert!(validate_segment_path("media/init.mp4").is_ok());
        assert!(validate_segment_path("chunklist_b3000000.m3u8").is_ok());
    }

    #[test]
    fn allows_single_dot_in_path() {
        // A single dot is not a traversal
        assert!(validate_segment_path("stream/segment.ts").is_ok());
        assert!(validate_segment_path("./current/seg.ts").is_ok());
    }

    #[test]
    fn allows_dot_dot_in_filename() {
        // "segment..ts" contains ".." but this IS traversal-adjacent and
        // should be blocked to be safe (defense in depth)
        assert!(validate_segment_path("segment..ts").is_err());
    }

    // --- percent_decode ---

    #[test]
    fn percent_decode_basic() {
        assert_eq!(percent_decode("hello%20world"), "hello world");
        assert_eq!(percent_decode("%2e%2e"), "..");
        assert_eq!(percent_decode("%2E%2F"), "./");
    }

    #[test]
    fn percent_decode_passthrough() {
        assert_eq!(percent_decode("noencode"), "noencode");
        assert_eq!(percent_decode(""), "");
    }

    #[test]
    fn percent_decode_invalid_sequence() {
        // Invalid hex digits after % should be left as-is
        assert_eq!(percent_decode("%ZZ"), "%ZZ");
        assert_eq!(percent_decode("%"), "%");
        assert_eq!(percent_decode("%2"), "%2");
    }

    // --- hex_val ---

    #[test]
    fn hex_val_digits() {
        assert_eq!(hex_val(b'0'), Some(0));
        assert_eq!(hex_val(b'9'), Some(9));
        assert_eq!(hex_val(b'a'), Some(10));
        assert_eq!(hex_val(b'f'), Some(15));
        assert_eq!(hex_val(b'A'), Some(10));
        assert_eq!(hex_val(b'F'), Some(15));
        assert_eq!(hex_val(b'g'), None);
        assert_eq!(hex_val(b' '), None);
    }
}
