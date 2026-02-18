pub mod handlers;
pub mod state;

use crate::config::Config;
use axum::{routing::get, Router};
use metrics_exporter_prometheus::PrometheusBuilder;
use state::AppState;
use tower_http::cors::CorsLayer;
use tracing::{error, info};

/// Start the Axum HTTP server
pub async fn start(config: Config) -> Result<(), Box<dyn std::error::Error>> {
    let port = config.port;
    let base_url = config.base_url.clone();

    // Install Prometheus metrics recorder
    let prometheus_handle = PrometheusBuilder::new()
        .install_recorder()
        .expect("Failed to install Prometheus recorder");
    info!("Prometheus metrics recorder installed");

    // Create shared application state
    let state = AppState::new(config);

    // Spawn background task for session cleanup (prevents memory leaks)
    let cleanup_sessions = state.sessions.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            interval.tick().await;
            let before = cleanup_sessions.session_count();
            cleanup_sessions.cleanup_expired();
            let after = cleanup_sessions.session_count();
            if before != after {
                info!(
                    "Session cleanup: removed {} expired sessions ({} active)",
                    before - after,
                    after
                );
            }
            // Update session gauge metric
            crate::metrics::set_active_sessions(after);
        }
    });

    // CORS: always permissive — Ritcher serves HLS playlists and segments
    // that must be accessible from any web player origin (HLS.js, video.js, etc.)
    info!("CORS: Permissive mode (required for HLS player access)");
    let cors = CorsLayer::very_permissive();

    // Build router with all routes
    let app = Router::new()
        .route("/", get(handlers::health::health_check))
        .route("/health", get(handlers::health::health_check))
        // Prometheus metrics endpoint
        .route(
            "/metrics",
            get({
                let handle = prometheus_handle.clone();
                move || handlers::metrics::serve_metrics(handle)
            }),
        )
        // Demo endpoints: synthetic playlist/manifest with ad signals for testing
        .route(
            "/demo/playlist.m3u8",
            get(handlers::demo::serve_demo_playlist),
        )
        .route(
            "/demo/manifest.mpd",
            get(handlers::demo::serve_demo_manifest),
        )
        // Stitcher endpoints
        .route(
            "/stitch/{session_id}/playlist.m3u8",
            get(handlers::playlist::serve_playlist),
        )
        .route(
            "/stitch/{session_id}/manifest.mpd",
            get(handlers::manifest::serve_manifest),
        )
        .route(
            "/stitch/{session_id}/segment/{*segment_path}",
            get(handlers::segment::serve_segment),
        )
        .route(
            "/stitch/{session_id}/ad/{ad_name}",
            get(handlers::ad::serve_ad),
        )
        .layer(cors)
        .with_state(state);

    // Bind TCP listener
    let addr = format!("0.0.0.0:{}", port);
    let listener = match tokio::net::TcpListener::bind(addr.as_str()).await {
        Ok(listener) => listener,
        Err(e) => {
            error!("Failed to bind to {}: {}. Is port {} already in use?", addr, e, port);
            return Err(e.into());
        }
    };

    info!("Server bound to {}", addr);
    info!("Public URL: {}", base_url);
    info!("  Health:  {}/health", base_url);
    info!("  Metrics: {}/metrics", base_url);
    info!(
        "  Demo:    {}/stitch/demo/playlist.m3u8?origin={}/demo/playlist.m3u8",
        base_url, base_url
    );

    // Start serving with graceful shutdown
    if let Err(e) = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
    {
        error!("Server error: {}", e);
        return Err(e.into());
    }

    info!("Server shut down gracefully");
    Ok(())
}

/// Wait for shutdown signal (Ctrl+C or SIGTERM)
async fn shutdown_signal() {
    use tokio::signal;

    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => info!("Received Ctrl+C, shutting down"),
        _ = terminate => info!("Received SIGTERM, shutting down"),
    }
}
