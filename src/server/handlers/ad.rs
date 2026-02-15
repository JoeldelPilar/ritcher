use crate::{error::Result, server::state::AppState};
use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
};
use tracing::info;

/// Serve ad segments
/// TODO: Replace hardcoded URL with ad decision service
pub async fn serve_ad(
    Path((session_id, ad_name)): Path<(String, String)>,
    State(state): State<AppState>,
) -> Result<Response> {
    info!("Serving ad: {} for session: {}", ad_name, session_id);

    // TODO: This should be replaced with proper ad decision logic
    // For now, using a hardcoded test ad URL
    let ad_url = "https://hls.src.tedm.io/content/ts_h264_480p_1s/playlist.m3u8";

    info!("Fetching ad from: {}", ad_url);

    // Fetch ad from ad server using shared HTTP client
    let response = state.http_client.get(ad_url).send().await?;

    if !response.status().is_success() {
        return Err(crate::error::RitcherError::OriginFetchError(
            response.error_for_status().unwrap_err(),
        ));
    }

    let bytes = response.bytes().await?;

    // Return ad segment with proper Content-Type header
    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "video/MP2T")],
        Body::from(bytes.to_vec()),
    )
        .into_response())
}
