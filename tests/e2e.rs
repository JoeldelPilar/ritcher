//! End-to-end tests for Ritcher SSAI stitcher
//!
//! Starts a real Axum server on a random port and tests the full
//! HTTP pipeline for both HLS and DASH endpoints.

use ritcher::config::{AdProviderType, Config, SessionStoreType, StitchingMode};
use ritcher::server::build_router;
use std::net::SocketAddr;

/// Start a test server on a random port and return its address (SSAI mode)
async fn start_test_server() -> SocketAddr {
    let config = Config {
        port: 0,
        base_url: "http://localhost".to_string(),
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
    };

    let app = build_router(config).await;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("Failed to bind test server");
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    addr
}

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
    let addr = start_test_server().await;
    let client = reqwest::Client::new();

    // Use the demo playlist as origin — self-contained, no external deps
    let origin = format!("http://{}/demo/playlist.m3u8", addr);
    let resp = client
        .get(format!(
            "http://{}/stitch/e2e-test/playlist.m3u8?origin={}",
            addr, origin
        ))
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
    let addr = start_test_server().await;
    let client = reqwest::Client::new();

    // Use the demo manifest as origin — self-contained
    let origin = format!("http://{}/demo/manifest.mpd", addr);
    let resp = client
        .get(format!(
            "http://{}/stitch/e2e-test/manifest.mpd?origin={}",
            addr, origin
        ))
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

// ── SGAI tests ───────────────────────────────────────────────────────────────

/// Start a test server in SGAI mode on a random port
async fn start_sgai_test_server() -> SocketAddr {
    let config = Config {
        port: 0,
        base_url: "http://localhost".to_string(),
        origin_url: "https://example.com".to_string(),
        is_dev: true,
        stitching_mode: StitchingMode::Sgai,
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

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("Failed to bind SGAI test server");
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    addr
}

#[tokio::test]
async fn sgai_hls_interstitials() {
    let addr = start_sgai_test_server().await;
    let client = reqwest::Client::new();

    // Use the demo playlist as origin (has EXT-X-PROGRAM-DATE-TIME + CUE markers)
    let origin = format!("http://{}/demo/playlist.m3u8", addr);
    let resp = client
        .get(format!(
            "http://{}/stitch/sgai-test/playlist.m3u8?origin={}",
            addr, origin
        ))
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

    let origin = format!("http://{}/demo/playlist.m3u8", addr);
    let resp = client
        .get(format!(
            "http://{}/stitch/ssai-regression/playlist.m3u8?origin={}",
            addr, origin
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
