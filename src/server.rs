use axum::{
    routing::get,
    Router,
};
use tracing::info;

pub async fn start() {
    // Build our application with routes
    let app = Router::new()
        .route("/", get(health_check))
        .route("/health", get(health_check));

    let addr = "0.0.0.0:3000";
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .unwrap();
    
    info!("🚀 Server listening on http://{}", addr);
    
    axum::serve(listener, app)
        .await
        .unwrap();
}

async fn health_check() -> &'static str {
    "🦀 Ritcher is running!"
}