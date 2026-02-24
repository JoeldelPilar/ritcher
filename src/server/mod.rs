pub mod handlers;
pub mod state;
pub mod url_validation;

use crate::config::Config;
use axum::{Router, routing::get};
use metrics_exporter_prometheus::PrometheusBuilder;
use state::AppState;
use tower_http::cors::CorsLayer;
use tracing::{error, info};

/// Build the Axum router with all routes and shared state
///
/// Extracted for testability — E2E tests use this to start a server
/// without the Prometheus recorder and startup logging.
pub async fn build_router(config: Config) -> Router {
    let state = AppState::new(config).await;

    // Spawn background task for session cleanup (prevents memory leaks)
    let cleanup_sessions = state.sessions.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            interval.tick().await;
            cleanup_sessions.cleanup_expired().await;
        }
    });

    // Spawn background task for ad cache eviction (TTL + size bound)
    let cleanup_ad_provider = state.ad_provider.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            interval.tick().await;
            cleanup_ad_provider.cleanup_cache();
        }
    });

    let cors = CorsLayer::very_permissive();

    Router::new()
        .route("/", get(handlers::health::health_check))
        .route("/health", get(handlers::health::health_check))
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
        .route(
            "/stitch/{session_id}/asset-list/{break_id}",
            get(handlers::asset_list::serve_asset_list),
        )
        .layer(cors)
        .with_state(state)
}

/// Start the Axum HTTP server
pub async fn start(config: Config) -> Result<(), Box<dyn std::error::Error>> {
    let port = config.port;
    let base_url = config.base_url.clone();

    // Install Prometheus metrics recorder
    let prometheus_handle = PrometheusBuilder::new()
        .install_recorder()
        .expect("Failed to install Prometheus recorder");
    info!("Prometheus metrics recorder installed");

    // Build the application router
    let app = build_router(config)
        .await
        // Prometheus metrics endpoint (only in production start, not in E2E tests)
        .route(
            "/metrics",
            get({
                let handle = prometheus_handle.clone();
                move || handlers::metrics::serve_metrics(handle)
            }),
        );

    // Bind TCP listener
    let addr = format!("0.0.0.0:{}", port);
    let listener = match tokio::net::TcpListener::bind(addr.as_str()).await {
        Ok(listener) => listener,
        Err(e) => {
            error!(
                "Failed to bind to {}: {}. Is port {} already in use?",
                addr, e, port
            );
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
