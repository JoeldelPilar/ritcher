use metrics::{counter, gauge, histogram};
use std::time::Instant;

// ── Metric names ────────────────────────────────────────────────────────

/// Total HTTP requests by endpoint and status
pub const REQUESTS_TOTAL: &str = "ritcher_requests_total";
/// Request duration in seconds
pub const REQUEST_DURATION: &str = "ritcher_request_duration_seconds";
/// Currently active sessions
pub const ACTIVE_SESSIONS: &str = "ritcher_active_sessions";
/// Ad breaks detected across all requests
pub const AD_BREAKS_DETECTED: &str = "ritcher_ad_breaks_detected";
/// VAST requests by result (success, error, timeout, empty)
pub const VAST_REQUESTS: &str = "ritcher_vast_requests_total";
/// Slate fallback activations
pub const SLATE_FALLBACKS: &str = "ritcher_slate_fallbacks_total";
/// Origin fetch errors
pub const ORIGIN_FETCH_ERRORS: &str = "ritcher_origin_fetch_errors_total";
/// Tracking beacons fired by event type and result
pub const TRACKING_BEACONS: &str = "ritcher_tracking_beacons_total";

// ── Recording helpers ───────────────────────────────────────────────────

/// Record an incoming request
pub fn record_request(endpoint: &str, status: u16) {
    counter!(REQUESTS_TOTAL, "endpoint" => endpoint.to_string(), "status" => status.to_string())
        .increment(1);
}

/// Record request duration
pub fn record_duration(endpoint: &str, start: Instant) {
    let duration = start.elapsed().as_secs_f64();
    histogram!(REQUEST_DURATION, "endpoint" => endpoint.to_string()).record(duration);
}

/// Update active session count
pub fn set_active_sessions(count: usize) {
    gauge!(ACTIVE_SESSIONS).set(count as f64);
}

/// Record detected ad breaks
pub fn record_ad_breaks(count: usize) {
    counter!(AD_BREAKS_DETECTED).increment(count as u64);
}

/// Record a VAST request result
pub fn record_vast_request(result: &str) {
    counter!(VAST_REQUESTS, "result" => result.to_string()).increment(1);
}

/// Record a slate fallback activation
pub fn record_slate_fallback() {
    counter!(SLATE_FALLBACKS).increment(1);
}

/// Record an origin fetch error
pub fn record_origin_error() {
    counter!(ORIGIN_FETCH_ERRORS).increment(1);
}

/// Record a tracking beacon event
pub fn record_tracking_event(event: &str, result: &str) {
    counter!(TRACKING_BEACONS, "event" => event.to_string(), "result" => result.to_string())
        .increment(1);
}

/// SGAI: total EXT-X-DATERANGE interstitial markers injected
pub const INTERSTITIALS_INJECTED: &str = "ritcher_interstitials_injected_total";
/// SGAI: asset-list requests by HTTP status
pub const ASSET_LIST_REQUESTS: &str = "ritcher_asset_list_requests_total";

/// Record injected interstitial markers
pub fn record_interstitials(count: usize) {
    counter!(INTERSTITIALS_INJECTED).increment(count as u64);
}

/// Record an asset-list request result
pub fn record_asset_list_request(status: u16) {
    counter!(ASSET_LIST_REQUESTS, "status" => status.to_string()).increment(1);
}
