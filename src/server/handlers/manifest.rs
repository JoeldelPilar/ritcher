use crate::{
    dash::{cue, interleaver, parser},
    error::Result,
    metrics,
    server::state::AppState,
};
use axum::{
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
};
use std::collections::HashMap;
use std::time::Instant;
use tracing::info;

/// Serve modified DASH manifest with stitched ad Periods
pub async fn serve_manifest(
    Path(session_id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
    State(state): State<AppState>,
) -> Result<Response> {
    let start = Instant::now();
    info!("Serving DASH manifest for session: {}", session_id);

    // Get origin URL from query params or fallback to config
    let origin_url = params
        .get("origin")
        .map(|s| s.as_str())
        .unwrap_or(&state.config.origin_url);

    info!("Fetching MPD from origin: {}", origin_url);

    // Fetch MPD from origin using shared HTTP client
    let response = state.http_client.get(origin_url).send().await.map_err(|e| {
        metrics::record_origin_error();
        crate::error::RitcherError::OriginFetchError(e)
    })?;

    if !response.status().is_success() {
        metrics::record_origin_error();
        metrics::record_request("manifest", 502);
        metrics::record_duration("manifest", start);
        return Err(crate::error::RitcherError::OriginFetchError(
            response.error_for_status().unwrap_err(),
        ));
    }

    let content = response.text().await?;

    // Parse DASH MPD
    let mut mpd = parser::parse_mpd(&content)?;

    // Extract base URL from origin
    let origin_base = origin_url
        .rsplit_once('/')
        .map(|(base, _)| base)
        .unwrap_or(origin_url);

    // Step 1: Detect ad breaks from EventStream/SCTE-35
    let ad_breaks = cue::detect_dash_ad_breaks(&mpd);

    if !ad_breaks.is_empty() {
        info!("Detected {} ad break(s)", ad_breaks.len());
        metrics::record_ad_breaks(ad_breaks.len());

        // Step 2: Get ad segments for each break
        let ad_segments_per_break: Vec<_> = ad_breaks
            .iter()
            .map(|ad_break| state.ad_provider.get_ad_segments(ad_break.duration as f32, &session_id))
            .collect();

        // Step 3: Interleave ad Periods into MPD
        mpd = interleaver::interleave_ads_mpd(
            mpd,
            &ad_breaks,
            &ad_segments_per_break,
            &session_id,
            &state.config.base_url,
        );
    } else {
        info!("No ad breaks detected in MPD");
    }

    // Step 4: Rewrite URLs to proxy through stitcher
    parser::rewrite_dash_urls(&mut mpd, &session_id, &state.config.base_url, origin_base)?;

    // Step 5: Serialize MPD to XML
    let mpd_xml = parser::serialize_mpd(&mpd)?;

    metrics::record_request("manifest", 200);
    metrics::record_duration("manifest", start);

    // Return MPD with proper Content-Type header
    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/dash+xml")],
        mpd_xml,
    )
        .into_response())
}
