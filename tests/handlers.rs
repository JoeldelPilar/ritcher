//! Handler-level tests using tower::ServiceExt::oneshot and wiremock.
//!
//! Tests the full Axum router (middleware + handlers) without binding a TCP
//! listener. Faster and more deterministic than E2E tests.
//!
//! Two testing modes are used:
//!
//! 1. `oneshot` (no TCP) — for tests that do not need a live origin server.
//!    Fast; used for session-ID validation, static demo endpoints, rate-limit.
//!
//! 2. `start_server` + `reqwest` — for tests that need a wiremock origin.
//!    A real TCP listener is bound first; the config's `origin_url` is set to
//!    the wiremock server so the SSRF validator (which blocks user-supplied
//!    `?origin=` pointing at 127.0.0.1) is never triggered.

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use ritcher::config::{AdProviderType, Config, SessionStoreType, StitchingMode};
use ritcher::server::build_router;
use std::net::SocketAddr;
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ── Test helpers ─────────────────────────────────────────────────────────────

/// Build a test config with sensible defaults.
///
/// `is_dev: true` disables the SSRF-safe DNS resolver so wiremock (on
/// 127.0.0.1) is reachable during tests.
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
        origin_timeout_secs: 30,
        manifest_cache_ttl_ms: 2000,
    }
}

/// Build a test config where `origin_url` points at the given mock server.
///
/// This is the correct pattern for wiremock-based tests: set the origin in
/// the Config (operator-trusted), never via `?origin=` query param (which the
/// SSRF validator would block for 127.x addresses).
fn config_with_origin(mock_server: &MockServer, path: &str) -> Config {
    let origin_url = format!("{}{}", mock_server.uri(), path);
    Config {
        origin_url,
        base_url: "http://localhost:3000".to_string(),
        is_dev: true,
        ..test_config()
    }
}

/// Build a test config where `origin_url` points at the mock server,
/// with a given stitching mode.
fn config_with_origin_and_mode(
    mock_server: &MockServer,
    path: &str,
    mode: StitchingMode,
) -> Config {
    Config {
        stitching_mode: mode,
        ..config_with_origin(mock_server, path)
    }
}

/// Spin up a real Axum server on a random port and return its socket address.
///
/// The server uses the given config. Bind the listener first so the port is
/// known before the server task starts.
async fn start_server(config: Config) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("Failed to bind test server");
    let addr = listener.local_addr().unwrap();
    let app = build_router(config).await;
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

// ── Minimal valid HLS playlist used across tests ──────────────────────────────

const MINIMAL_HLS: &str = r#"#EXTM3U
#EXT-X-VERSION:3
#EXT-X-TARGETDURATION:6
#EXTINF:6.0,
seg-001.ts
#EXT-X-ENDLIST
"#;

/// Minimal HLS with a CUE-OUT break so the stitcher has something to work on.
const HLS_WITH_CUE: &str = r#"#EXTM3U
#EXT-X-VERSION:3
#EXT-X-TARGETDURATION:6
#EXTINF:6.0,
seg-001.ts
#EXT-X-CUE-OUT:10
#EXTINF:6.0,
seg-002.ts
#EXT-X-CUE-IN
#EXTINF:6.0,
seg-003.ts
#EXT-X-ENDLIST
"#;

/// Minimal DASH MPD with an SCTE-35 EventStream ad signal.
const MINIMAL_MPD: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011"
     xmlns:scte35="urn:scte:scte35:2013:xml"
     type="static"
     mediaPresentationDuration="PT60S"
     minBufferTime="PT2S">
  <Period id="content" start="PT0S" duration="PT10S">
    <EventStream schemeIdUri="urn:scte:scte35:2013:xml" timescale="90000">
      <Event id="1" duration="900000">
        <scte35:SpliceInfoSection>
          <scte35:SpliceInsert spliceEventId="1" outOfNetworkIndicator="1"/>
        </scte35:SpliceInfoSection>
      </Event>
    </EventStream>
    <AdaptationSet mimeType="video/mp4" segmentAlignment="true">
      <Representation id="1" bandwidth="800000" codecs="avc1.42c01e" width="640" height="360">
        <BaseURL>video/</BaseURL>
        <SegmentList duration="4">
          <SegmentURL media="seg-1.m4s"/>
        </SegmentList>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>
"#;

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

// ── Playlist handler — session ID validation (oneshot, no origin needed) ────

/// `../hack` contains `/` which is rejected by `validate_session_id`.
#[tokio::test]
async fn playlist_invalid_session_id_returns_400() {
    let app = build_router(test_config()).await;

    // Axum path-extracts `../hack` when the URL is percent-encoded so the
    // router can actually match the route. Raw `..` in a URL path would cause
    // the HTTP client / server to normalise it away before reaching the handler.
    // We use a session ID with a disallowed character (dot) instead.
    let req = Request::builder()
        .uri("/stitch/bad.session/playlist.m3u8")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "Dots in session ID should return 400"
    );
}

#[tokio::test]
async fn playlist_session_id_with_slash_returns_404_not_matched() {
    // A session ID containing `/` makes the URL structurally ambiguous.
    // Axum treats `..%2Fhack` as a path segment and typically won't match
    // the route at all (404 or other mismatch). Either outcome is acceptable
    // — the handler must never process it.
    let app = build_router(test_config()).await;

    let req = Request::builder()
        .uri("/stitch/..%2Fhack/playlist.m3u8")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    // Could be 400 (if route matched and validator ran) or 404 (if route
    // didn't match). Either is safe — it must NOT be 200.
    assert_ne!(
        resp.status(),
        StatusCode::OK,
        "Path traversal attempt must not return 200"
    );
}

#[tokio::test]
async fn playlist_empty_session_id_not_routed() {
    // An empty session ID (/stitch//playlist.m3u8) does not match the Axum
    // route pattern — the router returns 404 rather than 400 because Axum
    // never dispatches to the handler at all.
    let app = build_router(test_config()).await;

    let req = Request::builder()
        .uri("/stitch//playlist.m3u8")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_ne!(
        resp.status(),
        StatusCode::OK,
        "Empty session ID must not return 200"
    );
}

#[tokio::test]
async fn playlist_session_id_too_long_returns_400() {
    let long_id = "a".repeat(65); // Exceeds 64-char limit
    let app = build_router(test_config()).await;

    let req = Request::builder()
        .uri(format!("/stitch/{}/playlist.m3u8", long_id))
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

// ── Playlist handler — origin error paths (wiremock) ─────────────────────────

/// Origin returns 404 → handler should return 502 (Bad Gateway) because it
/// maps non-success HTTP responses to `OriginFetchError` which yields 502.
#[tokio::test]
async fn playlist_origin_404_returns_502() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/playlist.m3u8"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&mock_server)
        .await;

    let addr = start_server(config_with_origin(&mock_server, "/playlist.m3u8")).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("http://{}/stitch/test-session/playlist.m3u8", addr))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        502,
        "Origin 404 should produce 502 Bad Gateway"
    );
}

/// Origin returns 500 → handler should return 502.
#[tokio::test]
async fn playlist_origin_500_returns_502() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/playlist.m3u8"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&mock_server)
        .await;

    let addr = start_server(config_with_origin(&mock_server, "/playlist.m3u8")).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("http://{}/stitch/test-session/playlist.m3u8", addr))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 502);
}

/// Origin returns 502 → handler should propagate as 502.
#[tokio::test]
async fn playlist_origin_502_returns_502() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/playlist.m3u8"))
        .respond_with(ResponseTemplate::new(502))
        .mount(&mock_server)
        .await;

    let addr = start_server(config_with_origin(&mock_server, "/playlist.m3u8")).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("http://{}/stitch/test-session/playlist.m3u8", addr))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 502);
}

/// Origin returns a valid HLS playlist → handler returns 200 with correct
/// Content-Type and valid M3U8 body.
#[tokio::test]
async fn playlist_valid_origin_returns_200_with_hls_content_type() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/playlist.m3u8"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(MINIMAL_HLS)
                .insert_header("content-type", "application/vnd.apple.mpegurl"),
        )
        .mount(&mock_server)
        .await;

    let addr = start_server(config_with_origin(&mock_server, "/playlist.m3u8")).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("http://{}/stitch/test-session/playlist.m3u8", addr))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
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

    let body = resp.text().await.unwrap();
    assert!(body.contains("#EXTM3U"), "Body should be valid M3U8");
}

/// SSAI mode: origin playlist with CUE-OUT break → stitched playlist has
/// DISCONTINUITY tags (ad segments interleaved).
#[tokio::test]
async fn playlist_ssai_mode_interleaves_ad_segments() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/playlist.m3u8"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(HLS_WITH_CUE)
                .insert_header("content-type", "application/vnd.apple.mpegurl"),
        )
        .mount(&mock_server)
        .await;

    let addr = start_server(config_with_origin_and_mode(
        &mock_server,
        "/playlist.m3u8",
        StitchingMode::Ssai,
    ))
    .await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("http://{}/stitch/ssai-test/playlist.m3u8", addr))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();

    // SSAI injects DISCONTINUITY markers around ad segments
    assert!(
        body.contains("#EXT-X-DISCONTINUITY"),
        "SSAI must inject DISCONTINUITY tags, got:\n{}",
        body
    );
    // Ad segments should be proxied through stitcher
    assert!(
        body.contains("/stitch/ssai-test/ad/"),
        "SSAI must rewrite ad segment URLs to stitcher proxy, got:\n{}",
        body
    );
}

/// SGAI mode: origin playlist with CUE-OUT break → stitched playlist has
/// EXT-X-DATERANGE interstitial tags (no segment replacement).
#[tokio::test]
async fn playlist_sgai_mode_injects_daterange_tags() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/playlist.m3u8"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(HLS_WITH_CUE)
                .insert_header("content-type", "application/vnd.apple.mpegurl"),
        )
        .mount(&mock_server)
        .await;

    let addr = start_server(config_with_origin_and_mode(
        &mock_server,
        "/playlist.m3u8",
        StitchingMode::Sgai,
    ))
    .await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("http://{}/stitch/sgai-test/playlist.m3u8", addr))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();

    assert!(
        body.contains("EXT-X-DATERANGE"),
        "SGAI must inject EXT-X-DATERANGE tags, got:\n{}",
        body
    );
    assert!(
        body.contains("com.apple.hls.interstitial"),
        "SGAI must include interstitial CLASS, got:\n{}",
        body
    );
    // SGAI must NOT inject DISCONTINUITY (segments are not replaced)
    assert!(
        !body.contains("#EXT-X-DISCONTINUITY"),
        "SGAI must not inject DISCONTINUITY tags, got:\n{}",
        body
    );
}

/// Origin returns a body that is not valid UTF-8 → handler returns 422.
#[tokio::test]
async fn playlist_non_utf8_body_returns_422() {
    let mock_server = MockServer::start().await;

    // Raw bytes that are not valid UTF-8
    let invalid_utf8 = vec![0xFF, 0xFE, 0x00, 0x01];

    Mock::given(method("GET"))
        .and(path("/playlist.m3u8"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(invalid_utf8)
                .insert_header("content-type", "application/vnd.apple.mpegurl"),
        )
        .mount(&mock_server)
        .await;

    let addr = start_server(config_with_origin(&mock_server, "/playlist.m3u8")).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("http://{}/stitch/test-session/playlist.m3u8", addr))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        422,
        "Non-UTF-8 body should return 422 Unprocessable Entity"
    );
}

/// Origin returns a body that is valid UTF-8 but not parseable as M3U8
/// → handler returns 422.
#[tokio::test]
async fn playlist_invalid_m3u8_body_returns_422() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/playlist.m3u8"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("this is not a valid m3u8 playlist at all")
                .insert_header("content-type", "application/vnd.apple.mpegurl"),
        )
        .mount(&mock_server)
        .await;

    let addr = start_server(config_with_origin(&mock_server, "/playlist.m3u8")).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("http://{}/stitch/test-session/playlist.m3u8", addr))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        422,
        "Invalid M3U8 body should return 422 Unprocessable Entity"
    );
}

// ── Manifest handler — session ID validation ──────────────────────────────────

#[tokio::test]
async fn manifest_invalid_session_id_returns_400() {
    let app = build_router(test_config()).await;

    let req = Request::builder()
        .uri("/stitch/bad.session/manifest.mpd")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn manifest_session_id_too_long_returns_400() {
    let long_id = "b".repeat(65);
    let app = build_router(test_config()).await;

    let req = Request::builder()
        .uri(format!("/stitch/{}/manifest.mpd", long_id))
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

// ── Manifest handler — origin error paths (wiremock) ─────────────────────────

/// Origin returns 500 → manifest handler returns 502.
#[tokio::test]
async fn manifest_origin_500_returns_502() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/manifest.mpd"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&mock_server)
        .await;

    let addr = start_server(config_with_origin(&mock_server, "/manifest.mpd")).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("http://{}/stitch/test-session/manifest.mpd", addr))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 502);
}

/// Origin returns 404 → manifest handler returns 502.
#[tokio::test]
async fn manifest_origin_404_returns_502() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/manifest.mpd"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&mock_server)
        .await;

    let addr = start_server(config_with_origin(&mock_server, "/manifest.mpd")).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("http://{}/stitch/test-session/manifest.mpd", addr))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 502);
}

/// Origin returns a valid DASH MPD → handler returns 200 with correct
/// Content-Type and valid XML body.
#[tokio::test]
async fn manifest_valid_origin_returns_200_with_dash_content_type() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/manifest.mpd"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(MINIMAL_MPD)
                .insert_header("content-type", "application/dash+xml"),
        )
        .mount(&mock_server)
        .await;

    let addr = start_server(config_with_origin(&mock_server, "/manifest.mpd")).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("http://{}/stitch/test-session/manifest.mpd", addr))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
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

    let body = resp.text().await.unwrap();
    assert!(body.contains("<MPD"), "Body should contain MPD element");
}

/// SSAI mode: origin MPD with SCTE-35 EventStream → stitched MPD has ad Periods.
#[tokio::test]
async fn manifest_ssai_mode_inserts_ad_periods() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/manifest.mpd"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(MINIMAL_MPD)
                .insert_header("content-type", "application/dash+xml"),
        )
        .mount(&mock_server)
        .await;

    let addr = start_server(config_with_origin_and_mode(
        &mock_server,
        "/manifest.mpd",
        StitchingMode::Ssai,
    ))
    .await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("http://{}/stitch/ssai-dash/manifest.mpd", addr))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();

    assert!(
        body.contains("ad-0"),
        "SSAI must insert an ad Period with id='ad-0', got:\n{}",
        body
    );
}

/// SGAI mode: origin MPD with SCTE-35 EventStream → stitched MPD has callback
/// EventStreams instead of ad Periods.
#[tokio::test]
async fn manifest_sgai_mode_injects_callback_eventstream() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/manifest.mpd"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(MINIMAL_MPD)
                .insert_header("content-type", "application/dash+xml"),
        )
        .mount(&mock_server)
        .await;

    let addr = start_server(config_with_origin_and_mode(
        &mock_server,
        "/manifest.mpd",
        StitchingMode::Sgai,
    ))
    .await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("http://{}/stitch/sgai-dash/manifest.mpd", addr))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();

    assert!(
        body.contains("urn:mpeg:dash:event:callback:2015"),
        "SGAI must inject callback EventStream, got:\n{}",
        body
    );
    assert!(
        !body.contains("\"ad-0\""),
        "SGAI must not inject ad Periods, got:\n{}",
        body
    );
}

/// Origin returns a body that is not valid XML/MPD → handler returns 422.
#[tokio::test]
async fn manifest_invalid_mpd_body_returns_422() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/manifest.mpd"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("this is not a valid MPD document")
                .insert_header("content-type", "application/dash+xml"),
        )
        .mount(&mock_server)
        .await;

    let addr = start_server(config_with_origin(&mock_server, "/manifest.mpd")).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("http://{}/stitch/test-session/manifest.mpd", addr))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        422,
        "Invalid MPD body should return 422 Unprocessable Entity"
    );
}

// ── Ad handler — session ID validation ───────────────────────────────────────

#[tokio::test]
async fn ad_handler_invalid_session_id_returns_400() {
    let app = build_router(test_config()).await;

    let req = Request::builder()
        .uri("/stitch/bad.session/ad/break-0-seg-0.ts")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

/// Unknown ad name (does not match "break-N-seg-M.ts" pattern) → 500.
///
/// `StaticAdProvider::resolve_segment_url` returns `None` for unknown names,
/// which the handler wraps as `InternalError` → 500.
#[tokio::test]
async fn ad_handler_unknown_ad_name_returns_500() {
    let app = build_router(test_config()).await;

    let req = Request::builder()
        .uri("/stitch/test-session/ad/not-a-valid-segment-name.ts")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::INTERNAL_SERVER_ERROR,
        "Unknown ad name should return 500 Internal Server Error"
    );
}

// ── Segment handler — session ID and path validation ─────────────────────────

#[tokio::test]
async fn segment_invalid_session_id_returns_400() {
    let app = build_router(test_config()).await;

    let req = Request::builder()
        .uri("/stitch/bad.session/segment/seg-001.ts")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

/// Path traversal in segment path → 400.
///
/// `validate_segment_path` in `handlers/segment.rs` rejects `..` components
/// and maps them to `InvalidOrigin` → 400 Bad Request.
#[tokio::test]
async fn segment_path_traversal_returns_400() {
    let app = build_router(test_config()).await;

    // Axum wildcard paths decode percent-encoding before the handler sees it,
    // but our validator also handles literal `..` in the path.
    let req = Request::builder()
        .uri("/stitch/test-session/segment/..%2Fetc%2Fpasswd")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    // 400 if the handler ran and rejected it; 404 if the router rejected the path.
    // Either is correct — it must NOT be 200.
    assert_ne!(
        resp.status(),
        StatusCode::OK,
        "Path traversal in segment path must not return 200"
    );
}

/// Literal double-dot in segment path → must be rejected.
#[tokio::test]
async fn segment_literal_dot_dot_returns_non_200() {
    let app = build_router(test_config()).await;

    let req = Request::builder()
        .uri("/stitch/test-session/segment/safe/../../../etc/passwd")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_ne!(
        resp.status(),
        StatusCode::OK,
        "Double-dot segment path must not return 200"
    );
}

// ── Segment handler — origin error paths (wiremock) ──────────────────────────

/// Segment origin returns 404 → handler returns 502 (OriginFetchError).
#[tokio::test]
async fn segment_origin_404_returns_502() {
    let mock_server = MockServer::start().await;

    // The origin serves segments under the same base URL
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&mock_server)
        .await;

    let addr = start_server(config_with_origin(&mock_server, "/stream")).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!(
            "http://{}/stitch/test-session/segment/seg-001.ts",
            addr
        ))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 502);
}

// ── Asset-list handler — session ID validation ────────────────────────────────

#[tokio::test]
async fn asset_list_invalid_session_id_returns_400() {
    let app = build_router(test_config()).await;

    let req = Request::builder()
        .uri("/stitch/bad.session/asset-list/0?dur=30")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

/// Valid asset-list request with Static provider → returns JSON with ASSETS array.
#[tokio::test]
async fn asset_list_valid_session_returns_200_with_json() {
    let app = build_router(test_config()).await;

    let req = Request::builder()
        .uri("/stitch/test-session/asset-list/0?dur=30")
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
        ct.contains("application/json"),
        "Expected JSON content-type, got: {}",
        ct
    );

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(
        json["ASSETS"].is_array(),
        "Response should contain an ASSETS array"
    );
    assert!(
        !json["ASSETS"].as_array().unwrap().is_empty(),
        "ASSETS array should not be empty"
    );
}

// ── Playlist handler — response size limit ────────────────────────────────

/// Origin advertises Content-Length above MAX_MANIFEST_SIZE → handler rejects
/// early without reading the body (Content-Length pre-check).
#[tokio::test]
async fn playlist_oversized_content_length_returns_502() {
    let mock_server = MockServer::start().await;

    // Return a small body but with a Content-Length header claiming 20 MB.
    // The handler should reject based on the header alone.
    Mock::given(method("GET"))
        .and(path("/playlist.m3u8"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("tiny")
                .insert_header("content-length", "20971520"), // 20 MB
        )
        .mount(&mock_server)
        .await;

    let addr = start_server(config_with_origin(&mock_server, "/playlist.m3u8")).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("http://{}/stitch/test-session/playlist.m3u8", addr))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        502,
        "Oversized Content-Length should return 502"
    );

    let body = resp.text().await.unwrap();
    assert!(
        !body.contains("20971520"),
        "Error body must not leak byte count, got: {}",
        body
    );
}

/// Origin sends a large chunked response (no Content-Length) → handler aborts
/// once the streaming body exceeds the limit. This is the critical fix: the
/// old code buffered the entire body before checking size.
#[tokio::test]
async fn playlist_oversized_chunked_body_returns_502() {
    let mock_server = MockServer::start().await;

    // Build a body that exceeds the 10 MB limit.
    // Use ~11 MB of data with no Content-Length header (chunked encoding).
    let oversized_body = "x".repeat(11 * 1024 * 1024);

    Mock::given(method("GET"))
        .and(path("/playlist.m3u8"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(oversized_body)
                .insert_header("content-type", "application/vnd.apple.mpegurl"),
        )
        .mount(&mock_server)
        .await;

    let addr = start_server(config_with_origin(&mock_server, "/playlist.m3u8")).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("http://{}/stitch/test-session/playlist.m3u8", addr))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        502,
        "Oversized chunked body should return 502"
    );
}

/// Origin sends a body just under the limit → handler accepts it and
/// proceeds to parse. Ensures the size limit is not overly restrictive.
#[tokio::test]
async fn playlist_body_under_limit_is_accepted() {
    let mock_server = MockServer::start().await;

    // A valid HLS playlist that is under 10 MB (just use the minimal one)
    Mock::given(method("GET"))
        .and(path("/playlist.m3u8"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(MINIMAL_HLS)
                .insert_header("content-type", "application/vnd.apple.mpegurl"),
        )
        .mount(&mock_server)
        .await;

    let addr = start_server(config_with_origin(&mock_server, "/playlist.m3u8")).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("http://{}/stitch/test-session/playlist.m3u8", addr))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        200,
        "Body under 10 MB limit should be accepted"
    );
}

// ── Manifest handler — response size limit ───────────────────────────────

/// Origin advertises oversized Content-Length for DASH manifest → 502.
#[tokio::test]
async fn manifest_oversized_content_length_returns_502() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/manifest.mpd"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("tiny")
                .insert_header("content-length", "20971520"), // 20 MB
        )
        .mount(&mock_server)
        .await;

    let addr = start_server(config_with_origin(&mock_server, "/manifest.mpd")).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("http://{}/stitch/test-session/manifest.mpd", addr))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        502,
        "Oversized Content-Length should return 502"
    );
}

/// Origin sends an oversized chunked DASH manifest → handler aborts mid-stream.
#[tokio::test]
async fn manifest_oversized_chunked_body_returns_502() {
    let mock_server = MockServer::start().await;

    let oversized_body = "x".repeat(11 * 1024 * 1024);

    Mock::given(method("GET"))
        .and(path("/manifest.mpd"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(oversized_body)
                .insert_header("content-type", "application/dash+xml"),
        )
        .mount(&mock_server)
        .await;

    let addr = start_server(config_with_origin(&mock_server, "/manifest.mpd")).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("http://{}/stitch/test-session/manifest.mpd", addr))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        502,
        "Oversized chunked body should return 502"
    );
}

/// Missing `dur` query param → defaults to 30.0 and still returns 200.
#[tokio::test]
async fn asset_list_missing_dur_defaults_to_30s() {
    let app = build_router(test_config()).await;

    let req = Request::builder()
        .uri("/stitch/test-session/asset-list/0")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "Missing dur param should default to 30s, not error"
    );
}

/// `X-VERIFICATIONS` field absent when Static provider returns no verifications.
#[tokio::test]
async fn asset_list_no_verifications_field_when_empty() {
    let app = build_router(test_config()).await;

    let req = Request::builder()
        .uri("/stitch/test-session/asset-list/0?dur=10")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json_str = String::from_utf8(body.to_vec()).unwrap();

    // Static provider never adds verifications — field must be absent
    assert!(
        !json_str.contains("X-VERIFICATIONS"),
        "Static provider should not emit X-VERIFICATIONS, got: {}",
        json_str
    );
}

// ── Origin error message does not leak internal URLs ─────────────────────────

/// When the origin returns an error, the response body must be a generic
/// message — not the raw reqwest error that includes the internal URL.
#[tokio::test]
async fn origin_error_body_does_not_leak_origin_url() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/playlist.m3u8"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&mock_server)
        .await;

    let addr = start_server(config_with_origin(&mock_server, "/playlist.m3u8")).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("http://{}/stitch/test-session/playlist.m3u8", addr))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 502);

    let body = resp.text().await.unwrap();
    // The body must not contain internal infrastructure details
    assert!(
        !body.contains("127.0.0.1"),
        "Error body must not leak internal IP, got: {}",
        body
    );
    assert!(
        !body.contains("localhost"),
        "Error body must not leak 'localhost', got: {}",
        body
    );
}
