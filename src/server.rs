use crate::config::Config;
use axum::{
    routing::get,
    Router,
    extract::{Path, Query, State},
    response::IntoResponse,
    http::StatusCode,
};
use tracing::{info, error};
use crate::stitcher::parser;
use std::collections::HashMap;
use reqwest;

pub async fn start(config: Config) -> Result<(), Box<dyn std::error::Error>> {
  let addr = format!("0.0.0.0:{}", config.port);

  let app = Router::new()
    .route("/", get(health_check))
    .route("/health", get(health_check))
    .route("/stitch/{session_id}/playlist.m3u8", get(serve_playlist))
    .route("/stitch/{session_id}/segment/{*segment_path}", get(serve_segment)).with_state(config);

  let listener = match tokio::net::TcpListener::bind(addr.as_str()).await {
    Ok(listener) => listener,
    Err(e) => {
      error!("Failed to bind to address {}: {}", addr, e);
      return Err(e.into());
    }
  };

  info!("🚀 Server listening on http://{}", addr);

  if let Err(e) = axum::serve(listener, app).await {
    error!("Server error: {}", e);
    return Err(e.into());
  }

  Ok(())
}

async fn health_check() -> &'static str {
    "🦀 Ritcher is running!"
}

async fn serve_playlist(
  Path(session_id): Path<String>,
  Query(params): Query<HashMap<String, String>>,
  State(config): State<Config>
) -> impl IntoResponse {
  info!("Serving playlist for session: {}", session_id);

  let origin_url = params.get("origin").map(|s| s.as_str()).unwrap_or(&config.origin_url);

  info!("fetching playlist from origin: {}", origin_url);

  let content = match reqwest::get(origin_url).await {
    Ok(response) => {
      if response.status().is_success() {
        match response.text().await {
          Ok(text) => text,
          Err(e) => {
            error!("Failed to read response text: {:?}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to read response text".to_string());
          }
        }
      } else {
        error!("origin server returned error: {}", response.status());
        return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to fetch playlist".to_string());
      }
    }
    Err(e) => {
      error!("Failed to fetch playlist from origin: {:?}", e);
      return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to fetch playlist from origin".to_string());
    }
  };

  let playlist = match parser::parse_hls_playlist(&content) {
    Ok(p) => p,
    Err(e) => {
      error!("Failed to parse playlist: {:?}", e);
      return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to parse playlist".to_string());
    }
  };

  let origin_base = origin_url.rsplit_once('/')
    .map(|(base, _)| base)
    .unwrap_or(origin_url);

  let modified_playlist = match parser::modify_playlist(playlist, &session_id, &config.base_url, origin_base) {
    Ok(p) => p,
    Err(e) => {
      error!("Failed to modify playlist: {:?}", e);
      return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to modify playlist".to_string());
    }
  };

  (StatusCode::OK, modified_playlist)
}

async fn serve_segment(
  Path((session_id, segment_path)): Path<(String, String)>,
  Query(params): Query<HashMap<String, String>>,
  State(config): State<Config>
) -> impl IntoResponse {
  info!("Serving segment: {} for session: {}", segment_path, session_id);

  let origin_base = params.get("origin").map(|s| s.as_str()).unwrap_or(&config.origin_url);
  
  let segment_url = format!("{}/{}", origin_base, segment_path);

  info!("fetching segment from origin: {}", segment_url);

  let bytes = match reqwest::get(&segment_url).await {
    Ok(response) => {
      if response.status().is_success() {
        match response.bytes().await {
          Ok(bytes) => bytes,
          Err(e) => {
            error!("Failed to read segment bytes: {:?}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, vec![]);
          }
        }
      } else {
        error!("Origin server returned error: {}", response.status());
        return (StatusCode::INTERNAL_SERVER_ERROR, vec![]);
      }
    }
    Err(e) => {
      error!("Failed to fetch segment from origin: {:?}", e);
      return (StatusCode::INTERNAL_SERVER_ERROR, vec![]);
    }
  };

  (StatusCode::OK, bytes.to_vec())
}