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
                tracing::error!("Origin fetch error: {:?}", e);
                (StatusCode::BAD_GATEWAY, self.to_string())
            }
            RitcherError::PlaylistParseError(ref e) => {
                tracing::error!("Playlist parse error: {}", e);
                (StatusCode::UNPROCESSABLE_ENTITY, self.to_string())
            }
            RitcherError::MpdParseError(ref e) => {
                tracing::error!("MPD parse error: {}", e);
                (StatusCode::UNPROCESSABLE_ENTITY, self.to_string())
            }
            RitcherError::PlaylistModifyError(ref e) => {
                tracing::error!("Playlist modify error: {}", e);
                (StatusCode::INTERNAL_SERVER_ERROR, self.to_string())
            }
            RitcherError::InvalidSessionId(ref e) => {
                tracing::error!("Invalid session ID: {}", e);
                (StatusCode::BAD_REQUEST, self.to_string())
            }
            RitcherError::ConfigError(ref e) => {
                tracing::error!("Configuration error: {}", e);
                (StatusCode::INTERNAL_SERVER_ERROR, self.to_string())
            }
            RitcherError::ConversionError(ref e) => {
                tracing::error!("Conversion error: {}", e);
                (StatusCode::INTERNAL_SERVER_ERROR, self.to_string())
            }
            RitcherError::InvalidOrigin(ref e) => {
                tracing::error!("Invalid origin URL: {}", e);
                (StatusCode::BAD_REQUEST, self.to_string())
            }
            RitcherError::InternalError(ref e) => {
                tracing::error!("Internal error: {}", e);
                (StatusCode::INTERNAL_SERVER_ERROR, self.to_string())
            }
        };

        (status, error_message).into_response()
    }
}

// Convenience type alias for Results
pub type Result<T> = std::result::Result<T, RitcherError>;
