use crate::{
    config::StitchingMode,
    dash::{cue, interleaver, parser, sgai},
    error::Result,
    metrics,
    server::{
        MAX_MANIFEST_SIZE,
        state::AppState,
        url_validation::{validate_origin_url, validate_session_id},
    },
};
use axum::{
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use futures_util::StreamExt;
use std::collections::HashMap;
use std::time::Instant;
use tracing::info;

/// Serve a modified DASH MPD with stitched ad Periods.
///
/// Fetches the origin MPD, detects SCTE-35 EventStream ad breaks, and
/// either inserts ad Periods (SSAI) or injects callback EventStreams (SGAI).
///
/// Returns `application/dash+xml` with HTTP 200 on success.
pub async fn serve_manifest(
    Path(session_id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
    State(state): State<AppState>,
) -> Result<Response> {
    validate_session_id(&session_id)?;
    let start = Instant::now();
    info!("Serving DASH manifest for session: {}", session_id);

    // Get origin URL from query params or fallback to config.
    // Validate user-supplied origin against SSRF attack vectors.
    let origin_url: &str = if let Some(origin) = params.get("origin") {
        validate_origin_url(origin)?;
        origin.as_str()
    } else {
        &state.config.origin_url
    };

    info!("Fetching MPD from origin: {}", origin_url);

    // Try manifest cache first, then fetch from origin
    let content = if let Some(cached) = state.manifest_cache.get(origin_url) {
        cached
    } else {
        let response = state
            .http_client
            .get(origin_url)
            .send()
            .await
            .map_err(|e| {
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

        // Check Content-Length header if present for early rejection
        if let Some(content_length) = response.content_length()
            && content_length > MAX_MANIFEST_SIZE
        {
            metrics::record_origin_error();
            return Err(crate::error::RitcherError::ResponseTooLarge(format!(
                "Content-Length {} bytes exceeds {} byte limit",
                content_length, MAX_MANIFEST_SIZE
            )));
        }

        // Stream body incrementally to enforce size limit.
        // Unlike `response.bytes()` which buffers the entire body before
        // the size check, this aborts as soon as the limit is exceeded —
        // protecting against chunked-encoding OOM attacks.
        let mut body_buf = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk_result) = stream.next().await {
            let chunk = chunk_result.map_err(crate::error::RitcherError::OriginFetchError)?;
            body_buf.extend_from_slice(&chunk);
            if body_buf.len() as u64 > MAX_MANIFEST_SIZE {
                metrics::record_origin_error();
                return Err(crate::error::RitcherError::ResponseTooLarge(format!(
                    "Response body exceeded {} byte limit while streaming",
                    MAX_MANIFEST_SIZE
                )));
            }
        }

        let body = String::from_utf8(body_buf).map_err(|e| {
            crate::error::RitcherError::MpdParseError(format!(
                "Response body is not valid UTF-8: {}",
                e
            ))
        })?;

        state.manifest_cache.insert(origin_url, body.clone());
        body
    };

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

        match state.config.stitching_mode {
            StitchingMode::Ssai => {
                // Step 2: Get ad segments for each break
                let mut ad_segments_per_break = Vec::with_capacity(ad_breaks.len());
                for ad_break in &ad_breaks {
                    // DASH ad break durations (f64) are typically < 300s; f32
                    // precision loss at that magnitude is negligible for ad fetching.
                    #[allow(clippy::cast_possible_truncation)]
                    let dur = ad_break.duration as f32;
                    let segs = state.ad_provider.get_ad_segments(dur, &session_id).await;
                    ad_segments_per_break.push(segs);
                }

                // Step 3: Interleave ad Periods into MPD
                mpd = interleaver::interleave_ads_mpd(
                    mpd,
                    &ad_breaks,
                    &ad_segments_per_break,
                    &session_id,
                    &state.config.base_url,
                );
            }
            StitchingMode::Sgai => {
                // SGAI: inject callback EventStreams instead of ad Periods
                sgai::inject_dash_callbacks(
                    &mut mpd,
                    &ad_breaks,
                    &session_id,
                    &state.config.base_url,
                );
                sgai::strip_scte35_event_streams(&mut mpd);
                metrics::record_interstitials(ad_breaks.len());
            }
        }
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
