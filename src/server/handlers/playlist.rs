use crate::{
    ad::{AdProvider, interleaver},
    config::StitchingMode,
    error::Result,
    hls::{cue, interstitial, parser},
    metrics,
    server::{state::AppState, url_validation::validate_origin_url},
};
use axum::{
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use m3u8_rs::Playlist;
use std::collections::HashMap;
use std::time::Instant;
use tracing::info;

/// Serve modified HLS playlist with stitched ad markers
pub async fn serve_playlist(
    Path(session_id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
    State(state): State<AppState>,
) -> Result<Response> {
    let start = Instant::now();
    info!("Serving playlist for session: {}", session_id);

    // Get origin URL from query params or fallback to config.
    // Validate user-supplied origin against SSRF attack vectors.
    let origin_url: &str = if let Some(origin) = params.get("origin") {
        validate_origin_url(origin)?;
        origin.as_str()
    } else {
        &state.config.origin_url
    };

    info!("Fetching playlist from origin: {}", origin_url);

    // Fetch playlist from origin using shared HTTP client
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
        metrics::record_request("playlist", 502);
        metrics::record_duration("playlist", start);
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

    // Determine track type from query params (set by master playlist rewrite for alternatives)
    let track_type = match params.get("track").map(|s| s.as_str()) {
        Some("audio") => "audio",
        Some("subtitles") => "subtitles",
        _ => "video",
    };

    // Process playlist through the ad insertion pipeline
    let modified_playlist = process_playlist(
        playlist,
        &session_id,
        &state.config.base_url,
        origin_base,
        state.ad_provider.as_ref(),
        track_type,
        &state.config.stitching_mode,
    )?;

    // Serialize to string
    let playlist_str = parser::serialize_playlist(modified_playlist)?;

    metrics::record_request("playlist", 200);
    metrics::record_duration("playlist", start);

    // Return playlist with proper Content-Type header
    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/vnd.apple.mpegurl")],
        playlist_str,
    )
        .into_response())
}

/// Process playlist through the ad insertion pipeline
///
/// The `track_type` parameter indicates the media track type:
/// - `"video"` — full ad insertion pipeline (default)
/// - `"audio"` — ad insertion if CUE markers present (muxed ads contain audio),
///   otherwise pass through unchanged
/// - `"subtitles"` — skip ad insertion entirely, only rewrite URLs
///
/// The `stitching_mode` selects the insertion strategy:
/// - `StitchingMode::Ssai` — replace content segments with ad segments (traditional SSAI)
/// - `StitchingMode::Sgai` — inject EXT-X-DATERANGE interstitial markers (HLS Interstitials)
fn process_playlist(
    playlist: Playlist,
    session_id: &str,
    base_url: &str,
    origin_base: &str,
    ad_provider: &dyn AdProvider,
    track_type: &str,
    stitching_mode: &StitchingMode,
) -> Result<Playlist> {
    // Handle MasterPlaylist: rewrite variant-stream URLs through stitcher
    if matches!(&playlist, Playlist::MasterPlaylist(_)) {
        info!("Processing master playlist — rewriting variant URLs");
        return parser::rewrite_master_urls(playlist, session_id, base_url, origin_base);
    }

    // Subtitle/CC tracks: skip ad insertion, only rewrite content URLs
    if track_type == "subtitles" {
        info!("Subtitle track — skipping ad insertion");
        return parser::rewrite_content_urls(playlist, session_id, base_url, origin_base);
    }

    // MediaPlaylist: full ad insertion pipeline
    let Playlist::MediaPlaylist(mut media_playlist) = playlist else {
        return Ok(playlist);
    };

    // Step 1: Detect ad breaks from CUE tags
    let ad_breaks = cue::detect_ad_breaks(&media_playlist);

    if !ad_breaks.is_empty() {
        info!(
            "Detected {} ad break(s) for {} track",
            ad_breaks.len(),
            track_type
        );
        metrics::record_ad_breaks(ad_breaks.len());

        match stitching_mode {
            StitchingMode::Ssai => {
                // Step 2: Get ad segments for each break
                // For audio tracks, the same muxed ad segments are used — the player
                // demuxes the audio track from the muxed container
                let ad_segments_per_break: Vec<_> = ad_breaks
                    .iter()
                    .map(|ad_break| ad_provider.get_ad_segments(ad_break.duration, session_id))
                    .collect();

                // Step 3: Interleave ads into playlist
                media_playlist = interleaver::interleave_ads(
                    media_playlist,
                    &ad_breaks,
                    &ad_segments_per_break,
                    session_id,
                    base_url,
                );
            }
            StitchingMode::Sgai => {
                // SGAI: inject EXT-X-DATERANGE interstitial markers
                // Ensure PDT is present (required by HLS Interstitials spec)
                interstitial::ensure_program_date_time(&mut media_playlist);
                // Inject DateRange tags for each ad break
                interstitial::inject_interstitials(
                    &mut media_playlist,
                    &ad_breaks,
                    session_id,
                    base_url,
                );
                metrics::record_interstitials(ad_breaks.len());
            }
        }
    } else if track_type == "audio" {
        // Audio rendition without CUE markers: pass through without ad insertion.
        // The muxed video ad segments already contain audio, but without CUE markers
        // we cannot determine where to insert them in the audio timeline.
        info!("Audio track has no CUE markers — passing through without ad insertion");
    } else {
        info!("No ad breaks detected in playlist");
    }

    // Step 4: Rewrite content URLs to proxy through stitcher
    // Note: in SGAI mode we still rewrite content URLs so segments flow through
    // the stitcher proxy (required for session-aware segment serving)
    let playlist = Playlist::MediaPlaylist(media_playlist);
    parser::rewrite_content_urls(playlist, session_id, base_url, origin_base)
}
