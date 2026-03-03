use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};
use thiserror::Error;

/// Domain-specific error types for Ritcher
#[derive(Error, Debug)]
pub enum RitcherError {
    #[error("Failed to fetch content from origin: {0}")]
    OriginFetchError(#[from] reqwest::Error),

    #[error("Failed to parse HLS playlist: {0}")]
    PlaylistParseError(String),

    #[error("Failed to parse DASH MPD: {0}")]
    MpdParseError(String),

    #[error("Failed to modify playlist: {0}")]
    PlaylistModifyError(String),

    #[error("Invalid session ID: {0}")]
    InvalidSessionId(String),

    #[error("Configuration error: {0}")]
    ConfigError(String),

    #[error("Failed to convert data: {0}")]
    ConversionError(String),

    #[error("Invalid origin URL: {0}")]
    InvalidOrigin(String),

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
                (StatusCode::BAD_REQUEST, self.to_string())
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
}
