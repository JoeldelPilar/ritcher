use crate::{error::Result, metrics, server::state::AppState};
use axum::{
    body::Body,
    extract::{Path, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use std::time::{Duration, Instant};
use tracing::{info, warn};

/// Serve ad segments by proxying from the configured ad source
///
/// The ad_name encodes the break and segment index (e.g. "break-0-seg-3.ts").
/// We delegate URL resolution to the AdProvider, keeping this handler decoupled
/// from ad source implementation details.
///
/// Includes 1 retry with 500ms backoff on fetch failure.
pub async fn serve_ad(
    Path((session_id, ad_name)): Path<(String, String)>,
    State(state): State<AppState>,
) -> Result<Response> {
    let start = Instant::now();
    info!("Serving ad: {} for session: {}", ad_name, session_id);

    // Resolve ad segment identifier to actual source URL via the provider
    let ad_url = state
        .ad_provider
        .resolve_segment_url(&ad_name)
        .ok_or_else(|| {
            crate::error::RitcherError::InternalError(format!(
                "Failed to resolve ad segment URL for: {}",
                ad_name
            ))
        })?;

    info!("Fetching ad segment from: {}", ad_url);

    // Fetch ad segment with retry logic (1 retry, 500ms backoff)
    let max_attempts = 2;
    let mut last_error = None;

    for attempt in 1..=max_attempts {
        match state.http_client.get(&ad_url).send().await {
            Ok(response) if response.status().is_success() => {
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

                return Ok((
                    StatusCode::OK,
                    [(header::CONTENT_TYPE, content_type.as_str())],
                    Body::from(bytes.to_vec()),
                )
                    .into_response());
            }
            Ok(response) => {
                warn!(
                    "Ad segment fetch returned status {} (attempt {}/{})",
                    response.status(),
                    attempt,
                    max_attempts
                );
                last_error = Some(response.error_for_status().unwrap_err());
            }
            Err(e) => {
                warn!(
                    "Ad segment fetch failed: {} (attempt {}/{})",
                    e, attempt, max_attempts
                );
                last_error = Some(e);
            }
        }

        // Retry backoff (skip on last attempt)
        if attempt < max_attempts {
            warn!("Retrying ad segment fetch in 500ms...");
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    metrics::record_request("ad", 502);
    metrics::record_duration("ad", start);

    Err(crate::error::RitcherError::OriginFetchError(
        last_error.expect("Should have error after all retries failed"),
    ))
}
