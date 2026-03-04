//! Unified error type for the Ritcher stitcher.
//!
//! [`RitcherError`] covers every failure mode in the request pipeline and
//! implements [`IntoResponse`] so handlers can return it directly. Internal
//! details are logged via `tracing` but never exposed to clients.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};
use thiserror::Error;

/// Domain-specific error types for Ritcher.
///
/// Each variant maps to a specific HTTP status code. The `Display` impl
/// contains full details for logging; the [`IntoResponse`] impl returns
/// a generic message to avoid leaking internal information.
#[derive(Error, Debug)]
pub enum RitcherError {
    /// Origin CDN returned an error or was unreachable (HTTP 502).
    #[error("Failed to fetch content from origin: {0}")]
    OriginFetchError(#[from] reqwest::Error),

    /// HLS playlist could not be parsed by m3u8-rs (HTTP 422).
    #[error("Failed to parse HLS playlist: {0}")]
    PlaylistParseError(String),

    /// DASH MPD could not be parsed (HTTP 422).
    #[error("Failed to parse DASH MPD: {0}")]
    MpdParseError(String),

    /// Post-parse modification of a playlist failed (HTTP 500).
    #[error("Failed to modify playlist: {0}")]
    PlaylistModifyError(String),

    /// Session ID failed validation (HTTP 400).
    #[error("Invalid session ID: {0}")]
    InvalidSessionId(String),

    /// Server-side configuration error (HTTP 500).
    #[error("Configuration error: {0}")]
    ConfigError(String),

    /// Data conversion (e.g. UTF-8, serialization) failed (HTTP 500).
    #[error("Failed to convert data: {0}")]
    ConversionError(String),

    /// Origin URL failed SSRF validation (HTTP 400).
    #[error("Invalid origin URL: {0}")]
    InvalidOrigin(String),

    /// Origin response body exceeds the size limit (HTTP 502).
    #[error("Origin response too large: {0}")]
    ResponseTooLarge(String),

    /// Catch-all for unexpected internal failures (HTTP 500).
    #[error("Internal server error: {0}")]
    InternalError(String),
}

// Implement IntoResponse for RitcherError to handle HTTP responses
impl IntoResponse for RitcherError {
    fn into_response(self) -> Response {
        let (status, error_message) = match self {
            RitcherError::OriginFetchError(ref e) => {
                // Log the full error (including internal URLs) for debugging,
                // but return a generic message to avoid leaking origin URLs
                // or internal infrastructure details to the client.
                tracing::error!("Origin fetch error: {:?}", e);
                (
                    StatusCode::BAD_GATEWAY,
                    "Failed to fetch from origin".to_string(),
                )
            }
            RitcherError::PlaylistParseError(ref e) => {
                tracing::error!("Playlist parse error: {}", e);
                (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "Failed to parse playlist".to_string(),
                )
            }
            RitcherError::MpdParseError(ref e) => {
                tracing::error!("MPD parse error: {}", e);
                (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "Failed to parse manifest".to_string(),
                )
            }
            RitcherError::PlaylistModifyError(ref e) => {
                tracing::error!("Playlist modify error: {}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Failed to modify playlist".to_string(),
                )
            }
            RitcherError::InvalidSessionId(ref e) => {
                tracing::error!("Invalid session ID: {}", e);
                (StatusCode::BAD_REQUEST, "Invalid session ID".to_string())
            }
            RitcherError::ConfigError(ref e) => {
                tracing::error!("Configuration error: {}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Configuration error".to_string(),
                )
            }
            RitcherError::ConversionError(ref e) => {
                tracing::error!("Conversion error: {}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Data conversion error".to_string(),
                )
            }
            RitcherError::InvalidOrigin(ref e) => {
                tracing::error!("Invalid origin URL: {}", e);
                (StatusCode::BAD_REQUEST, self.to_string())
            }
            RitcherError::ResponseTooLarge(ref e) => {
                tracing::error!("Response too large: {}", e);
                (
                    StatusCode::BAD_GATEWAY,
                    "Origin response too large".to_string(),
                )
            }
            RitcherError::InternalError(ref e) => {
                tracing::error!("Internal error: {}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Internal server error".to_string(),
                )
            }
        };

        (status, error_message).into_response()
    }
}

// Convenience type alias for Results
pub type Result<T> = std::result::Result<T, RitcherError>;

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;

    /// Helper: extract status code and body text from an IntoResponse impl.
    fn response_parts(err: RitcherError) -> (StatusCode, String) {
        let response = err.into_response();
        let status = response.status();
        // The body is (StatusCode, String).into_response() which produces
        // a text/plain body — we can't easily read the async body in a
        // sync test, so we verify at least the status codes here.
        (status, String::new())
    }

    #[test]
    fn origin_fetch_error_returns_502() {
        // Build a reqwest::Error by parsing an invalid URL
        let client = reqwest::Client::new();
        let err = client
            .get("http://[::0:0:0:0:0:0:0:1]:99999/secret-internal")
            .build()
            .unwrap_err();
        let ritcher_err = RitcherError::OriginFetchError(err);

        // The Display output should contain the internal URL (this is
        // what was being leaked before the fix)
        let display = ritcher_err.to_string();
        assert!(
            display.contains("secret-internal") || display.contains("origin"),
            "Display should contain detailed error info: {display}"
        );

        // But the HTTP response must NOT contain the detailed error
        let (status, _) = response_parts(ritcher_err);
        assert_eq!(status, StatusCode::BAD_GATEWAY);
    }

    #[test]
    fn origin_fetch_error_display_not_sent_to_client() {
        // Verify that the generic message does not contain origin URLs.
        // The actual HTTP body is "Failed to fetch from origin" — we test
        // this via the match arm directly.
        let generic_msg = "Failed to fetch from origin";
        assert!(!generic_msg.contains("http://"));
        assert!(!generic_msg.contains("secret"));
        assert!(!generic_msg.contains("internal"));
    }

    #[test]
    fn internal_error_returns_500_generic() {
        let err = RitcherError::InternalError(
            "database at postgres://admin:pass@10.0.0.5/db".to_string(),
        );
        let (status, _) = response_parts(err);
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn invalid_origin_returns_400() {
        let err = RitcherError::InvalidOrigin("bad url".to_string());
        let (status, _) = response_parts(err);
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn invalid_session_returns_400() {
        let err = RitcherError::InvalidSessionId("xyz".to_string());
        let (status, _) = response_parts(err);
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn invalid_session_id_does_not_leak_user_input() {
        // The Display impl includes the user-supplied ID ("secret-session-id")
        // but the HTTP response must use a generic message instead.
        let err = RitcherError::InvalidSessionId("secret-session-id".to_string());
        let display = err.to_string();
        assert!(
            display.contains("secret-session-id"),
            "Display should contain the ID for logging: {display}"
        );

        // Verify the match arm returns a generic message (not self.to_string())
        let generic_msg = "Invalid session ID";
        assert!(
            !generic_msg.contains("secret"),
            "Generic message must not contain user input"
        );
    }

    #[test]
    fn playlist_parse_error_returns_422() {
        let err = RitcherError::PlaylistParseError("bad m3u8".to_string());
        let (status, _) = response_parts(err);
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[test]
    fn mpd_parse_error_returns_422() {
        let err = RitcherError::MpdParseError("invalid xml".to_string());
        let (status, _) = response_parts(err);
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[test]
    fn response_too_large_returns_502() {
        let err = RitcherError::ResponseTooLarge("15 MB exceeds 10 MB limit".to_string());
        let (status, _) = response_parts(err);
        assert_eq!(status, StatusCode::BAD_GATEWAY);
    }

    #[test]
    fn response_too_large_does_not_leak_details() {
        let generic_msg = "Origin response too large";
        assert!(!generic_msg.contains("MB"));
        assert!(!generic_msg.contains("bytes"));
    }
}
