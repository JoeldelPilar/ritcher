use crate::{error::Result, server::state::AppState};
use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
};
use std::collections::HashMap;
use tracing::info;

/// Proxy video segments from origin to player
pub async fn serve_segment(
    Path((session_id, segment_path)): Path<(String, String)>,
    Query(params): Query<HashMap<String, String>>,
    State(state): State<AppState>,
) -> Result<Response> {
    info!("Serving segment: {} for session: {}", segment_path, session_id);

    // Get origin base URL from query params or fallback to config
    let origin_base = params
        .get("origin")
        .map(|s| s.as_str())
        .unwrap_or(&state.config.origin_url);

    let segment_url = format!("{}/{}", origin_base, segment_path);

    info!("Fetching segment from origin: {}", segment_url);

    // Fetch segment from origin using shared HTTP client
    let response = state.http_client.get(&segment_url).send().await?;

    if !response.status().is_success() {
        return Err(crate::error::RitcherError::OriginFetchError(
            response.error_for_status().unwrap_err(),
        ));
    }

    let bytes = response.bytes().await?;

    // Return segment with proper Content-Type header for MPEG-TS
    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "video/MP2T")],
        Body::from(bytes.to_vec()),
    )
        .into_response())
}
