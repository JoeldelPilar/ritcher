//! Ritcher -- High-performance HLS/DASH SSAI/SGAI ad insertion stitcher.
//!
//! Ritcher sits between a video player and an origin CDN, dynamically
//! inserting ads into HLS and DASH manifests. It supports two stitching
//! modes:
//!
//! - **SSAI** (Server-Side Ad Insertion): replaces content segments with ad
//!   segments in the manifest before serving it to the player.
//! - **SGAI** (Server-Guided Ad Insertion): injects `EXT-X-DATERANGE`
//!   interstitial markers (HLS) or callback EventStreams (DASH) so the
//!   player fetches and plays ads client-side.
//!
//! # Crate layout
//!
//! - [`ad`] -- Ad decision logic (VAST parsing, interleaving, tracking, slate)
//! - [`cache`] -- Short-TTL origin manifest cache
//! - [`config`] -- Environment-based configuration
//! - [`dash`] -- DASH MPD parsing, SCTE-35 detection, period insertion
//! - [`error`] -- Unified error type ([`RitcherError`](error::RitcherError))
//! - [`hls`] -- HLS playlist parsing, CUE detection, SGAI interstitials
//! - [`http_retry`] -- HTTP fetch with exponential backoff
//! - [`metrics`] -- Prometheus metric definitions and recording helpers
//! - [`server`] -- Axum routes, handlers, middleware, state
//! - [`session`] -- Per-viewer session management (memory or Valkey)
//!
//! This crate exposes the library interface for benchmarks and integration
//! tests. The binary entry point is in `main.rs`.

#![warn(clippy::cast_possible_truncation)]
#![warn(clippy::significant_drop_tightening)]
#![warn(clippy::manual_let_else)]

pub mod ad;
pub mod cache;
pub mod config;
pub mod dash;
pub mod error;
pub mod hls;
pub mod http_retry;
pub mod metrics;
pub mod server;
pub mod session;
