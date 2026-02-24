use crate::{
    ad::tracking,
    error::Result,
    http_retry::{RetryConfig, fetch_with_retry},
    metrics,
    server::state::AppState,
};
use axum::{
    body::Body,
    extract::{Path, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use std::time::Instant;
use tracing::info;

/// Serve ad segments by proxying from the configured ad source
///
/// The ad_name encodes the break and segment index (e.g. "break-0-seg-3.ts").
/// We delegate URL resolution to the AdProvider, keeping this handler decoupled
/// from ad source implementation details.
///
/// Uses [`fetch_with_retry`] for fault-tolerant HTTP fetching.
pub async fn serve_ad(
    Path((session_id, ad_name)): Path<(String, String)>,
    State(state): State<AppState>,
) -> Result<Response> {
    let start = Instant::now();
    info!("Serving ad: {} for session: {}", ad_name, session_id);

    // Resolve ad segment with tracking context
    let resolved = state
        .ad_provider
        .resolve_segment_with_tracking(&ad_name, &session_id)
        .ok_or_else(|| {
            crate::error::RitcherError::InternalError(format!(
                "Failed to resolve ad segment URL for: {}",
                ad_name
            ))
        })?;

    // Fire tracking beacons (non-blocking) if present
    if let Some(tracking) = &resolved.tracking {
        // Fire impressions on first segment
        if tracking.segment_index == 0 {
            tracking::fire_impressions(state.http_client.clone(), &tracking.impression_urls);
        }

        // Fire quartile events
        let events = tracking::events_for_segment(
            tracking.segment_index,
            tracking.total_segments,
            &tracking.tracking_events,
        );
        for event in events {
            tracking::fire_beacon(
                state.http_client.clone(),
                event.url.clone(),
                event.event.clone(),
            );
        }
    }

    let ad_url = &resolved.url;
    info!("Fetching ad segment from: {}", ad_url);

    match fetch_with_retry(&state.http_client, ad_url, &RetryConfig::default()).await {
        Ok(response) => {
            let content_type = response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("video/MP2T")
                .to_string();

            let bytes = response.bytes().await?;
            info!("Ad segment {} fetched: {} bytes", ad_name, bytes.len());

            metrics::record_request("ad", 200);
            metrics::record_duration("ad", start);

            Ok((
                StatusCode::OK,
                [(header::CONTENT_TYPE, content_type.as_str())],
                Body::from(bytes.to_vec()),
            )
                .into_response())
        }
        Err(e) => {
            // Fire error beacon if tracking metadata is present
            if let Some(tracking) = &resolved.tracking
                && let Some(error_url) = &tracking.error_url
            {
                tracking::fire_error(state.http_client.clone(), error_url);
            }

            metrics::record_request("ad", 502);
            metrics::record_duration("ad", start);

            Err(crate::error::RitcherError::OriginFetchError(e))
        }
    }
}
