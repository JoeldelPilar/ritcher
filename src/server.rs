use axum::{
    routing::get,
    Router,
    extract::Path,
    response::IntoResponse,
    http::StatusCode,
};
use tracing::{info, error};
use crate::stitcher::parser;

pub async fn start() -> Result<(), Box<dyn std::error::Error>> {
  let addr = "0.0.0.0:3000";

  let app = Router::new()
    .route("/", get(health_check))
    .route("/health", get(health_check))
    .route("/stitch/{session_id}/playlist.m3u8", get(serve_playlist));

  let listener = match tokio::net::TcpListener::bind(addr).await {
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

async fn serve_playlist(Path(session_id): Path<String>) -> impl IntoResponse {
  info!("Serving playlist for session: {}", session_id);

  // for now: read test file
  let content = match std::fs::read_to_string("test-data/sample_playlist.m3u8") {
    Ok(c) => c,
    Err(e) => {
      error!("Failed to read file: {:?}", e);
      return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to read file".to_string());
    }
  };

  let playlist = match parser::parse_hls_playlist(&content) {
    Ok(p) => p,
    Err(e) => {
      error!("Failed to parse playlist: {:?}", e);
      return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to parse playlist".to_string());
    }
  };

  let modified_playlist = match parser::modify_playlist(playlist, &session_id) {
    Ok(p) => p,
    Err(e) => {
      error!("Failed to modify playlist: {:?}", e);
      return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to modify playlist".to_string());
    }
  };

  (StatusCode::OK, modified_playlist)
}