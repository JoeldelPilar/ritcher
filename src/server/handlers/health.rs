use crate::server::state::AppState;
use axum::{Json, extract::State, response::IntoResponse};
use serde::Serialize;

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Health check response
#[derive(Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
    pub version: &'static str,
    /// Approximation on Valkey backend (SCAN-based, not exact under concurrent
    /// writes); exact on the in-memory backend. Diagnostics only.
    pub active_sessions: usize,
    pub uptime_seconds: u64,
}

/// Health check endpoint returning structured JSON diagnostics
pub async fn health_check(State(state): State<AppState>) -> impl IntoResponse {
    let uptime = state.started_at.elapsed().as_secs();

    Json(HealthResponse {
        status: "ok",
        version: VERSION,
        active_sessions: state.sessions.session_count().await,
        uptime_seconds: uptime,
    })
}
