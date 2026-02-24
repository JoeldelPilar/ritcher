//! End-to-end tests for Ritcher SSAI stitcher
//!
//! Starts a real Axum server on a random port and tests the full
//! HTTP pipeline for both HLS and DASH endpoints.
//!
//! SSRF note: E2E tests bind the listener first to discover the port, then set
//! `origin_url` in config to the server's own demo endpoint. This avoids
//! passing `?origin=http://127.0.0.1:PORT/...` as a query parameter (which the
//! SSRF validator correctly blocks). Config-sourced origins are operator-trusted
//! and not subject to user-supplied origin validation.

use ritcher::config::{AdProviderType, Config, SessionStoreType, StitchingMode};
use ritcher::server::build_router;
use std::net::SocketAddr;

// ── Test server helpers ───────────────────────────────────────────────────────

/// Spin up a test server with the given stitching mode and origin demo path.
///
/// Binds a listener first to discover the random port, then configures
/// `origin_url` to point to the server's own demo endpoint. This avoids
/// user-supplied `?origin=` params (which the SSRF validator would block).
async fn start_server(mode: StitchingMode, origin_path: &str) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("Failed to bind test server");
    let addr = listener.local_addr().unwrap();

    let config = Config {
        port: 0,
        base_url: format!("http://{}", addr),
        origin_url: format!("http://{}{}", addr, origin_path),
        is_dev: true,
        stitching_mode: mode,
        ad_provider_type: AdProviderType::Static,
        ad_source_url: "https://hls.src.tedm.io/content/ts_h264_480p_1s".to_string(),
        ad_segment_duration: 1.0,
        vast_endpoint: None,
        slate_url: None,
        slate_segment_duration: 1.0,
        session_store: SessionStoreType::Memory,
        valkey_url: None,
        session_ttl_secs: 300,
    };

    let app = build_router(config).await;

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    addr
}

/// SSAI server with HLS demo playlist as origin.
async fn start_test_server() -> SocketAddr {
    start_server(StitchingMode::Ssai, "/demo/playlist.m3u8").await
}

/// SSAI server with DASH demo manifest as origin.
async fn start_dash_test_server() -> SocketAddr {
    start_server(StitchingMode::Ssai, "/demo/manifest.mpd").await
}

/// SGAI server with HLS demo playlist as origin.
async fn start_sgai_test_server() -> SocketAddr {
    start_server(StitchingMode::Sgai, "/demo/playlist.m3u8").await
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn health_check() {
    let addr = start_test_server().await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("http://{}/health", addr))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "ok");
}

#[tokio::test]
async fn demo_hls_playlist() {
    let addr = start_test_server().await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("http://{}/demo/playlist.m3u8", addr))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers().get("content-type").unwrap(),
        "application/vnd.apple.mpegurl"
    );
    let body = resp.text().await.unwrap();
    assert!(body.contains("#EXTM3U"));
    assert!(body.contains("#EXT-X-CUE-OUT:30"));
}

#[tokio::test]
async fn demo_dash_manifest() {
    let addr = start_test_server().await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("http://{}/demo/manifest.mpd", addr))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers().get("content-type").unwrap(),
        "application/dash+xml"
    );
    let body = resp.text().await.unwrap();
    assert!(body.contains("<MPD"));
    assert!(body.contains("EventStream"));
    assert!(body.contains("urn:scte:scte35:2013:xml"));
}

#[tokio::test]
async fn hls_stitch_pipeline() {
    // Config origin_url points to demo playlist — no ?origin= query param needed.
    // Passing localhost via ?origin= would be rejected by the SSRF validator.
    let addr = start_test_server().await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("http://{}/stitch/e2e-test/playlist.m3u8", addr))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();

    // Verify it's still a valid HLS playlist
    assert!(body.contains("#EXTM3U"));
    // Verify ad insertion happened (DISCONTINUITY = ads were interleaved)
    assert!(
        body.contains("#EXT-X-DISCONTINUITY"),
        "Expected DISCONTINUITY tags from ad interleaving, got:\n{}",
        body
    );
}

#[tokio::test]
async fn dash_stitch_pipeline() {
    // Uses a dedicated server with DASH demo as config origin.
    let addr = start_dash_test_server().await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("http://{}/stitch/e2e-test/manifest.mpd", addr))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();

    // Verify it's valid DASH MPD
    assert!(body.contains("<MPD"), "Expected MPD root element");
    // Verify ad Period was inserted
    assert!(
        body.contains("ad-0"),
        "Expected ad Period 'ad-0' from interleaving, got:\n{}",
        body
    );
}

// ── SGAI tests ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn sgai_hls_interstitials() {
    let addr = start_sgai_test_server().await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("http://{}/stitch/sgai-test/playlist.m3u8", addr))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();

    assert!(body.contains("#EXTM3U"), "Should be valid HLS");
    assert!(
        body.contains("EXT-X-DATERANGE"),
        "Expected EXT-X-DATERANGE from SGAI interstitial injection, got:\n{}",
        body
    );
    assert!(
        body.contains("com.apple.hls.interstitial"),
        "Expected CLASS=com.apple.hls.interstitial, got:\n{}",
        body
    );
    // SGAI does not replace segments — no DISCONTINUITY tags
    assert!(
        !body.contains("EXT-X-DISCONTINUITY"),
        "SGAI should not inject DISCONTINUITY tags, got:\n{}",
        body
    );
}

#[tokio::test]
async fn sgai_asset_list_endpoint() {
    let addr = start_sgai_test_server().await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!(
            "http://{}/stitch/sgai-test/asset-list/0?dur=30",
            addr
        ))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    assert!(
        resp.headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap()
            .contains("application/json"),
        "Content-Type should be application/json"
    );

    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(
        body["ASSETS"].is_array(),
        "Response should have ASSETS array"
    );
    assert!(
        !body["ASSETS"].as_array().unwrap().is_empty(),
        "ASSETS array should not be empty"
    );
}

#[tokio::test]
async fn ssai_mode_unchanged() {
    // Regression: SSAI pipeline must be unaffected by the SGAI additions
    let addr = start_test_server().await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!(
            "http://{}/stitch/ssai-regression/playlist.m3u8",
            addr
        ))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();

    assert!(body.contains("#EXTM3U"));
    assert!(
        body.contains("#EXT-X-DISCONTINUITY"),
        "SSAI should still inject DISCONTINUITY tags, got:\n{}",
        body
    );
    // No SGAI markers should appear in SSAI mode
    assert!(
        !body.contains("com.apple.hls.interstitial"),
        "SSAI mode must not include interstitial markers, got:\n{}",
        body
    );
}
