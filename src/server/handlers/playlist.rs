use crate::{error::Result, hls::parser, server::state::AppState};
use axum::{
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
};
use std::collections::HashMap;
use tracing::info;

/// Serve modified HLS playlist with stitched ad markers
pub async fn serve_playlist(
    Path(session_id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
    State(state): State<AppState>,
) -> Result<Response> {
    info!("Serving playlist for session: {}", session_id);

    // Get origin URL from query params or fallback to config
    let origin_url = params
        .get("origin")
        .map(|s| s.as_str())
        .unwrap_or(&state.config.origin_url);

    info!("Fetching playlist from origin: {}", origin_url);

    // Fetch playlist from origin using shared HTTP client
    let response = state.http_client.get(origin_url).send().await?;

    if !response.status().is_success() {
        return Err(crate::error::RitcherError::OriginFetchError(
            response.error_for_status().unwrap_err(),
        ));
    }

    let content = response.text().await?;

    // Parse HLS playlist
    let playlist = parser::parse_hls_playlist(&content)?;

    // Extract base URL from origin
    let origin_base = origin_url
        .rsplit_once('/')
        .map(|(base, _)| base)
        .unwrap_or(origin_url);

    // Modify playlist with stitcher URLs
    let modified_playlist =
        parser::modify_playlist(playlist, &session_id, &state.config.base_url, origin_base)?;

    // Return playlist with proper Content-Type header
    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/vnd.apple.mpegurl")],
        modified_playlist,
    )
        .into_response())
}
