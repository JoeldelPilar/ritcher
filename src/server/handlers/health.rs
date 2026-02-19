use crate::server::state::AppState;
use axum::{Json, extract::State, response::IntoResponse};
use serde::Serialize;

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Health check response
#[derive(Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
    pub version: &'static str,
    pub active_sessions: usize,
    pub uptime_seconds: u64,
}

/// Health check endpoint returning structured JSON diagnostics
pub async fn health_check(State(state): State<AppState>) -> impl IntoResponse {
    let uptime = state.started_at.elapsed().as_secs();

    Json(HealthResponse {
        status: "ok",
        version: VERSION,
        active_sessions: state.sessions.session_count(),
        uptime_seconds: uptime,
    })
}
