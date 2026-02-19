use crate::{error::Result, metrics, server::state::AppState};
use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tracing::{info, warn};

/// Proxy video segments from origin to player
///
/// Includes 1 retry with 500ms backoff on fetch failure.
pub async fn serve_segment(
    Path((session_id, segment_path)): Path<(String, String)>,
    Query(params): Query<HashMap<String, String>>,
    State(state): State<AppState>,
) -> Result<Response> {
    let start = Instant::now();
    info!(
        "Serving segment: {} for session: {}",
        segment_path, session_id
    );

    // Get origin base URL from query params or fallback to config
    let origin_base = params
        .get("origin")
        .map(|s| s.as_str())
        .unwrap_or(&state.config.origin_url);

    let segment_url = format!("{}/{}", origin_base, segment_path);

    info!("Fetching segment from origin: {}", segment_url);

    // Fetch segment with retry logic (1 retry, 500ms backoff)
    let max_attempts = 2;
    let mut last_error = None;

    for attempt in 1..=max_attempts {
        match state.http_client.get(&segment_url).send().await {
            Ok(response) if response.status().is_success() => {
                let content_type = response
                    .headers()
                    .get(header::CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("video/MP2T")
                    .to_string();

                let bytes = response.bytes().await?;

                metrics::record_request("segment", 200);
                metrics::record_duration("segment", start);

                return Ok((
                    StatusCode::OK,
                    [(header::CONTENT_TYPE, content_type.as_str())],
                    Body::from(bytes.to_vec()),
                )
                    .into_response());
            }
            Ok(response) => {
                warn!(
                    "Segment fetch returned status {} (attempt {}/{})",
                    response.status(),
                    attempt,
                    max_attempts
                );
                last_error = Some(response.error_for_status().unwrap_err());
            }
            Err(e) => {
                warn!(
                    "Segment fetch failed: {} (attempt {}/{})",
                    e, attempt, max_attempts
                );
                last_error = Some(e);
            }
        }

        if attempt < max_attempts {
            warn!("Retrying segment fetch in 500ms...");
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    metrics::record_origin_error();
    metrics::record_request("segment", 502);
    metrics::record_duration("segment", start);

    Err(crate::error::RitcherError::OriginFetchError(
        last_error.expect("Should have error after all retries failed"),
    ))
}
