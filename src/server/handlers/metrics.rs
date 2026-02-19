use axum::{
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use metrics_exporter_prometheus::PrometheusHandle;

/// Serve Prometheus metrics in text exposition format
///
/// Returns all registered metrics in the standard Prometheus text format
/// for scraping by Prometheus, Grafana Agent, or similar collectors.
pub async fn serve_metrics(handle: PrometheusHandle) -> Response {
    let metrics = handle.render();

    (
        StatusCode::OK,
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        metrics,
    )
        .into_response()
}
