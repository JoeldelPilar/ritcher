//! Handler-level tests using tower::ServiceExt::oneshot.
//!
//! Tests the full Axum router (middleware + handlers) without binding a TCP
//! listener. Faster and more deterministic than E2E tests.

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use ritcher::config::{AdProviderType, Config, SessionStoreType, StitchingMode};
use ritcher::server::build_router;
use tower::ServiceExt;

/// Build a test config with sensible defaults.
fn test_config() -> Config {
    Config {
        port: 0,
        base_url: "http://localhost:3000".to_string(),
        origin_url: "https://example.com".to_string(),
        is_dev: true,
        stitching_mode: StitchingMode::Ssai,
        ad_provider_type: AdProviderType::Static,
        ad_source_url: "https://hls.src.tedm.io/content/ts_h264_480p_1s".to_string(),
        ad_segment_duration: 1.0,
        vast_endpoint: None,
        slate_url: None,
        slate_segment_duration: 1.0,
        session_store: SessionStoreType::Memory,
        valkey_url: None,
        session_ttl_secs: 300,
        rate_limit_rpm: 0,
        demo_ad_base_url: None,
    }
}

// ── Health endpoint ─────────────────────────────────────────────────────────

#[tokio::test]
async fn health_returns_200_with_json() {
    let app = build_router(test_config()).await;

    let req = Request::builder()
        .uri("/health")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["status"], "ok");
    assert!(json["version"].is_string());
    assert!(json["active_sessions"].is_number());
    assert!(json["uptime_seconds"].is_number());
}

// ── Version header ──────────────────────────────────────────────────────────

#[tokio::test]
async fn all_responses_include_version_header() {
    let app = build_router(test_config()).await;

    let req = Request::builder()
        .uri("/health")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    let version = resp
        .headers()
        .get("x-ritcher-version")
        .expect("missing X-Ritcher-Version header");

    assert_eq!(version.to_str().unwrap(), env!("CARGO_PKG_VERSION"));
}

// ── 404 for unknown routes ──────────────────────────────────────────────────

#[tokio::test]
async fn unknown_route_returns_404() {
    let app = build_router(test_config()).await;

    let req = Request::builder()
        .uri("/nonexistent")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ── Demo endpoints ──────────────────────────────────────────────────────────

#[tokio::test]
async fn demo_hls_returns_valid_m3u8() {
    let app = build_router(test_config()).await;

    let req = Request::builder()
        .uri("/demo/playlist.m3u8")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let ct = resp
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(
        ct.contains("mpegurl"),
        "Expected HLS content-type, got: {}",
        ct
    );

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains("#EXTM3U"), "Response should be valid HLS");
}

#[tokio::test]
async fn demo_dash_returns_valid_mpd() {
    let app = build_router(test_config()).await;

    let req = Request::builder()
        .uri("/demo/manifest.mpd")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let ct = resp
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(
        ct.contains("dash+xml"),
        "Expected DASH content-type, got: {}",
        ct
    );

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains("<MPD"), "Response should be valid DASH MPD");
}

// ── Rate limiting ───────────────────────────────────────────────────────────

#[tokio::test]
async fn rate_limiter_blocks_after_limit() {
    let mut config = test_config();
    config.rate_limit_rpm = 3; // Very low limit for testing

    let app = build_router(config).await;

    // Router implements Clone — clone before each oneshot call.
    for _ in 0..3 {
        let req = Request::builder()
            .uri("/health")
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    // 4th request from same IP should be rate-limited
    let req = Request::builder()
        .uri("/health")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
}

// ── Root route aliases ──────────────────────────────────────────────────────

#[tokio::test]
async fn root_path_returns_health() {
    let app = build_router(test_config()).await;

    let req = Request::builder().uri("/").body(Body::empty()).unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["status"], "ok");
}
