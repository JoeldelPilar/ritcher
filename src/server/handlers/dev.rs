use crate::server::state::AppState;
use axum::{
    extract::State,
    http::{StatusCode, header},
    response::IntoResponse,
};

/// Dev UI HTML page, embedded at compile time from `assets/dev-ui.html`.
const DEV_UI_HTML: &str = include_str!("../../../assets/dev-ui.html");

/// Serve the dev UI dashboard.
///
/// Returns the embedded HTML page with `Content-Type: text/html; charset=utf-8`.
/// Returns 404 when `DEV_MODE` is not enabled, preventing accidental exposure
/// in production deployments.
pub async fn serve_dev_ui(State(state): State<AppState>) -> impl IntoResponse {
    if !state.config.is_dev {
        return StatusCode::NOT_FOUND.into_response();
    }

    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        DEV_UI_HTML,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_ui_html_contains_html_tag() {
        assert!(
            DEV_UI_HTML.contains("<html"),
            "dev-ui.html must contain an <html> tag"
        );
    }
}
