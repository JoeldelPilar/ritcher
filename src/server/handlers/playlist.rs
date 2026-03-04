use crate::{
    ad::{AdProvider, interleaver},
    config::StitchingMode,
    error::Result,
    hls::{cue, interstitial, ll_hls, parser},
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
use m3u8_rs::Playlist;
use std::collections::HashMap;
use std::time::Instant;
use tracing::info;

/// Serve a modified HLS playlist with stitched ad markers.
///
/// Fetches the origin playlist, detects SCTE-35 CUE ad breaks, and either
/// interleaves ad segments (SSAI) or injects `EXT-X-DATERANGE` interstitial
/// markers (SGAI). LL-HLS query parameters are forwarded to the origin.
///
/// Returns `application/vnd.apple.mpegurl` with HTTP 200 on success.
pub async fn serve_playlist(
    Path(session_id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
    State(state): State<AppState>,
) -> Result<Response> {
    validate_session_id(&session_id)?;
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

    // Validate LL-HLS numeric params before forwarding.
    validate_ll_hls_params(&params)?;

    // Forward LL-HLS query params (_HLS_msn, _HLS_part, etc.) to origin
    // so the origin can block until the requested MSN/part is available.
    let fetch_url = append_ll_hls_params(origin_url, &params);
    let is_blocking_reload = params.contains_key("_HLS_msn");

    info!("Fetching playlist from origin: {}", fetch_url);

    // Try manifest cache first, then fetch from origin.
    // Bypass cache for LL-HLS blocking requests — the origin long-polls
    // until the requested MSN/part is ready, so cached data would be stale.
    let cached = if is_blocking_reload {
        None
    } else {
        state.manifest_cache.get(origin_url)
    };

    let content = if let Some(cached) = cached {
        cached
    } else {
        let response = state
            .http_client
            .get(fetch_url.as_str())
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
            crate::error::RitcherError::PlaylistParseError(format!(
                "Response body is not valid UTF-8: {}",
                e
            ))
        })?;

        // Only cache non-blocking responses (LL-HLS blocking results are ephemeral)
        if !is_blocking_reload {
            state.manifest_cache.insert(origin_url, body.clone());
        }
        body
    };

    // Extract LL-HLS tags before m3u8-rs parsing — the parser drops
    // playlist-level unknown tags (SERVER-CONTROL, PART-INF, SKIP).
    let ll_tags = if ll_hls::is_ll_hls(&content) {
        info!("LL-HLS playlist detected — extracting tags for re-injection");
        Some(ll_hls::extract_ll_hls_tags(&content))
    } else {
        None
    };

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
    )
    .await?;

    // Serialize to string
    let mut playlist_str = parser::serialize_playlist(modified_playlist)?;

    // LL-HLS post-processing: re-inject tags that m3u8-rs dropped during parsing,
    // then rewrite partial segment and rendition report URIs through the stitcher.
    if let Some(ref tags) = ll_tags {
        playlist_str = ll_hls::inject_ll_hls_tags(&playlist_str, tags);
        playlist_str = ll_hls::rewrite_ll_hls_uris(
            &playlist_str,
            &session_id,
            &state.config.base_url,
            origin_base,
        );
        info!("LL-HLS post-processing complete");
    }

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
async fn process_playlist(
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
                let mut ad_segments_per_break = Vec::with_capacity(ad_breaks.len());
                for ad_break in &ad_breaks {
                    let segs = ad_provider
                        .get_ad_segments(ad_break.duration, session_id)
                        .await;
                    ad_segments_per_break.push(segs);
                }

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

/// Maximum allowed value for `_HLS_msn` and `_HLS_part` query parameters.
///
/// Acts as a sanity check -- no real playlist should have a media sequence
/// number or part index exceeding 1 million.
const MAX_LL_HLS_VALUE: u64 = 1_000_000;

/// Validate `_HLS_msn` and `_HLS_part` query parameters if present.
///
/// Both must be parseable as `u64` and not exceed [`MAX_LL_HLS_VALUE`].
/// Returns HTTP 400 if validation fails.
fn validate_ll_hls_params(params: &HashMap<String, String>) -> Result<()> {
    for key in &["_HLS_msn", "_HLS_part"] {
        if let Some(val) = params.get(*key) {
            let parsed: u64 = val.parse().map_err(|_| {
                crate::error::RitcherError::InvalidOrigin(format!(
                    "Invalid {} value: must be a non-negative integer",
                    key
                ))
            })?;
            if parsed > MAX_LL_HLS_VALUE {
                return Err(crate::error::RitcherError::InvalidOrigin(format!(
                    "Invalid {} value: exceeds maximum {}",
                    key, MAX_LL_HLS_VALUE
                )));
            }
        }
    }
    Ok(())
}

/// Append `_HLS_*` query parameters to an origin URL for LL-HLS blocking reload.
///
/// LL-HLS players send `_HLS_msn`, `_HLS_part`, `_HLS_push`, and `_HLS_skip`
/// query parameters. The stitcher must forward these verbatim to the origin
/// so it can block until the requested media sequence / part is ready.
///
/// Returns `origin_url` unchanged if no `_HLS_*` params are present.
fn append_ll_hls_params(origin_url: &str, params: &HashMap<String, String>) -> String {
    let ll_params: Vec<String> = params
        .iter()
        .filter(|(k, _)| k.starts_with("_HLS_"))
        .map(|(k, v)| format!("{}={}", k, v))
        .collect();

    if ll_params.is_empty() {
        return origin_url.to_string();
    }

    let sep = if origin_url.contains('?') { "&" } else { "?" };
    format!("{}{}{}", origin_url, sep, ll_params.join("&"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_ll_hls_params_accepts_valid_values() {
        let mut params = HashMap::new();
        params.insert("_HLS_msn".to_string(), "100".to_string());
        params.insert("_HLS_part".to_string(), "3".to_string());
        assert!(validate_ll_hls_params(&params).is_ok());
    }

    #[test]
    fn validate_ll_hls_params_accepts_zero() {
        let mut params = HashMap::new();
        params.insert("_HLS_msn".to_string(), "0".to_string());
        assert!(validate_ll_hls_params(&params).is_ok());
    }

    #[test]
    fn validate_ll_hls_params_accepts_max_value() {
        let mut params = HashMap::new();
        params.insert("_HLS_msn".to_string(), "1000000".to_string());
        assert!(validate_ll_hls_params(&params).is_ok());
    }

    #[test]
    fn validate_ll_hls_params_rejects_non_numeric_msn() {
        let mut params = HashMap::new();
        params.insert("_HLS_msn".to_string(), "abc".to_string());
        assert!(validate_ll_hls_params(&params).is_err());
    }

    #[test]
    fn validate_ll_hls_params_rejects_non_numeric_part() {
        let mut params = HashMap::new();
        params.insert("_HLS_part".to_string(), "xyz".to_string());
        assert!(validate_ll_hls_params(&params).is_err());
    }

    #[test]
    fn validate_ll_hls_params_rejects_negative_values() {
        let mut params = HashMap::new();
        params.insert("_HLS_msn".to_string(), "-1".to_string());
        assert!(validate_ll_hls_params(&params).is_err());
    }

    #[test]
    fn validate_ll_hls_params_rejects_exceeding_max() {
        let mut params = HashMap::new();
        params.insert("_HLS_msn".to_string(), "1000001".to_string());
        assert!(validate_ll_hls_params(&params).is_err());
    }

    #[test]
    fn validate_ll_hls_params_ignores_other_params() {
        let mut params = HashMap::new();
        params.insert("origin".to_string(), "https://example.com".to_string());
        params.insert("_HLS_skip".to_string(), "YES".to_string());
        // _HLS_skip is not validated (string param, not numeric)
        assert!(validate_ll_hls_params(&params).is_ok());
    }

    #[test]
    fn validate_ll_hls_params_empty_is_ok() {
        let params = HashMap::new();
        assert!(validate_ll_hls_params(&params).is_ok());
    }

    #[test]
    fn append_ll_hls_params_no_params() {
        let params = HashMap::new();
        let url = append_ll_hls_params("https://origin.example.com/live.m3u8", &params);
        assert_eq!(url, "https://origin.example.com/live.m3u8");
    }

    #[test]
    fn append_ll_hls_params_with_msn() {
        let mut params = HashMap::new();
        params.insert("_HLS_msn".to_string(), "42".to_string());
        let url = append_ll_hls_params("https://origin.example.com/live.m3u8", &params);
        assert!(url.contains("_HLS_msn=42"));
        assert!(url.contains('?'));
    }

    #[test]
    fn append_ll_hls_params_ignores_non_hls() {
        let mut params = HashMap::new();
        params.insert("origin".to_string(), "https://example.com".to_string());
        let url = append_ll_hls_params("https://origin.example.com/live.m3u8", &params);
        assert_eq!(url, "https://origin.example.com/live.m3u8");
    }

    #[test]
    fn append_ll_hls_params_existing_query() {
        let mut params = HashMap::new();
        params.insert("_HLS_msn".to_string(), "10".to_string());
        let url = append_ll_hls_params("https://origin.example.com/live.m3u8?token=abc", &params);
        assert!(url.contains("&_HLS_msn=10"));
    }
}
