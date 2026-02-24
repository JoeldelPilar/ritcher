//! HLS Interstitials asset-list endpoint
//!
//! Called by HLS players (hls.js ≥1.6, AVPlayer) when they encounter an
//! `EXT-X-DATERANGE` tag with `CLASS="com.apple.hls.interstitial"` and
//! `X-ASSET-LIST` pointing to this endpoint.
//!
//! Returns a JSON asset list conforming to RFC 8216bis §6.3:
//! ```json
//! {"ASSETS": [{"URI": "https://ad-cdn.example.com/ad.m3u8", "DURATION": 30.0}]}
//! ```

use crate::{error::Result, metrics, server::state::AppState};
use axum::{
    Json,
    extract::{Path, Query, State},
    response::{IntoResponse, Response},
};
use serde::Serialize;
use std::collections::HashMap;
use std::time::Instant;
use tracing::info;

/// HLS Interstitials asset-list response
#[derive(Serialize)]
struct AssetList {
    #[serde(rename = "ASSETS")]
    assets: Vec<Asset>,
}

/// Single asset entry in the asset-list
#[derive(Serialize)]
struct Asset {
    #[serde(rename = "URI")]
    uri: String,
    #[serde(rename = "DURATION")]
    duration: f64,
}

/// Serve HLS Interstitials asset-list JSON
///
/// Called by the player for each ad break it encounters. Returns the list of
/// ad creatives (URI + duration) the player should fetch and play inline.
///
/// Query params:
/// - `dur` — requested ad break duration in seconds (default: 30.0)
pub async fn serve_asset_list(
    Path((session_id, break_id)): Path<(String, String)>,
    Query(params): Query<HashMap<String, String>>,
    State(state): State<AppState>,
) -> Result<Response> {
    let start = Instant::now();
    info!(
        "Serving asset-list for session: {} break: {}",
        session_id, break_id
    );

    let duration: f32 = params
        .get("dur")
        .and_then(|d| d.parse().ok())
        .unwrap_or(30.0);

    let creatives = state.ad_provider.get_ad_creatives(duration, &session_id);

    let assets: Vec<Asset> = creatives
        .into_iter()
        .map(|c| Asset {
            uri: c.uri,
            duration: c.duration,
        })
        .collect();

    info!(
        "Asset-list: {} creative(s) for session {} (duration {}s)",
        assets.len(),
        session_id,
        duration
    );

    metrics::record_asset_list_request(200);
    metrics::record_duration("asset_list", start);

    Ok(Json(AssetList { assets }).into_response())
}
