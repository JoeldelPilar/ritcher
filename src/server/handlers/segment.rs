use crate::{
    error::Result,
    http_retry::{RetryConfig, fetch_with_retry},
    metrics,
    server::{state::AppState, url_validation::validate_origin_url},
};
use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use std::collections::HashMap;
use std::time::Instant;
use tracing::info;

/// Proxy video segments from origin to player
///
/// Includes 1 retry with 500ms backoff on fetch failure.
pub async fn serve_segment(
    Path((session_id, segment_path)): Path<(String, String)>,
    Query(params): Query<HashMap<String, String>>,
    State(state): State<AppState>,
) -> Result<Response> {
    let start = Instant::now();
    info!(
        "Serving segment: {} for session: {}",
        segment_path, session_id
    );

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

            let bytes = response.bytes().await?;

            metrics::record_request("segment", 200);
            metrics::record_duration("segment", start);

            Ok((
                StatusCode::OK,
                [(header::CONTENT_TYPE, content_type.as_str())],
                Body::from(bytes.to_vec()),
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
