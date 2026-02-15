pub mod handlers;
pub mod state;

use crate::config::Config;
use axum::{routing::get, Router};
use state::AppState;
use tracing::{error, info};

/// Start the Axum HTTP server
pub async fn start(config: Config) -> Result<(), Box<dyn std::error::Error>> {
    let addr = format!("0.0.0.0:{}", config.port);

    // Create shared application state
    let state = AppState::new(config);

    // Build router with all routes
    let app = Router::new()
        .route("/", get(handlers::health::health_check))
        .route("/health", get(handlers::health::health_check))
        .route(
            "/stitch/:session_id/playlist.m3u8",
            get(handlers::playlist::serve_playlist),
        )
        .route(
            "/stitch/:session_id/segment/*segment_path",
            get(handlers::segment::serve_segment),
        )
        .route(
            "/stitch/:session_id/ad/:ad_name",
            get(handlers::ad::serve_ad),
        )
        .with_state(state);

    // Bind TCP listener
    let listener = match tokio::net::TcpListener::bind(addr.as_str()).await {
        Ok(listener) => listener,
        Err(e) => {
            error!("Failed to bind to address {}: {}", addr, e);
            return Err(e.into());
        }
    };

    info!("🚀 Server listening on http://{}", addr);

    // Start serving
    if let Err(e) = axum::serve(listener, app).await {
        error!("Server error: {}", e);
        return Err(e.into());
    }

    Ok(())
}
