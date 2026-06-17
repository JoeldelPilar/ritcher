# Ritcher — Overview

Ritcher is a server-side HLS/DASH ad-insertion stitcher written in Rust. It sits between an **Origin** CDN (or packager) and a video **Player**, rewriting manifests to splice ads into live and VOD streams.

## Insertion modes

**SSAI (Server-Side Ad Insertion)** — Ritcher fetches ad segments and rewrites the Playlist or MPD so the Player sees one continuous stream. Content segments inside an ad break window are replaced with ad segments. `EXT-X-DISCONTINUITY` tags mark the boundaries.

**SGAI (Server-Guided Ad Insertion)** — Ritcher injects markers into the manifest instead of splicing media:
- **HLS**: `EXT-X-DATERANGE` with `CLASS="com.apple.hls.interstitial"` (RFC 8216bis). The Player fetches an asset-list JSON from Ritcher and plays the ad creatives client-side.
- **DASH**: a `urn:mpeg:dash:event:callback:2015` EventStream pointing at the same asset-list endpoint.

Both modes work with any ad provider. The active mode is controlled by `STITCHING_MODE`.

## Supported stream formats

| Format | Variant | Notes |
|--------|---------|-------|
| HLS | Master playlist | Variant-stream URLs rewritten through proxy |
| HLS | Media playlist (SSAI) | CUE-OUT/IN/CONT detection, ad interleaving |
| HLS | Media playlist (SGAI) | `EXT-X-DATERANGE` injection |
| HLS | LL-HLS | Partial segments, `EXT-X-PART`, blocking-reload query strings passed through |
| DASH | MPD (SSAI) | Period-based ad insertion after SCTE-35 EventStream |
| DASH | MPD (SGAI) | Callback EventStream injection |

## Domain glossary

The terms below have specific meanings in this codebase. See [CONTEXT.md](../CONTEXT.md) for the full glossary with disambiguation notes.

| Term | Definition |
|------|-----------|
| **Stitcher** | The Ritcher server process |
| **Origin** | Upstream CDN/packager serving the unmodified source stream |
| **Player** | Client software (hls.js, Shaka, AVPlayer, ExoPlayer) |
| **Manifest** | Umbrella for HLS Playlist and DASH MPD |
| **Playlist** | HLS-specific manifest (`.m3u8`) |
| **MPD** | DASH-specific manifest (`.mpd`) |
| **Ad break** | Time window in the source stream signaled for ad insertion (`AdBreak` / `DashAdBreak`) |
| **Ad segment** | One TS/CMAF chunk of an ad creative, used in SSAI (`AdSegment`) |
| **Ad creative** | Complete ad unit (HLS playlist URL or MP4 URL) served in SGAI asset list (`AdCreative`) |
| **Asset list** | JSON at `/stitch/{id}/asset-list/{break_id}` listing ad creatives per break |
| **SCTE-35** | Industry standard for ad-break signaling |
| **CUE marker** | HLS tags carrying SCTE-35 ad-break boundaries (`EXT-X-CUE-OUT`, `EXT-X-CUE-IN`, `EXT-X-CUE-OUT-CONT`) |
| **Interstitial** | HLS ad break expressed as `EXT-X-DATERANGE` per RFC 8216bis |
| **Slate** | Filler content shown when VAST returns no ads |
| **Tracking beacon** | URL fired server-side on ad-segment delivery to report VAST events |
| **Session** | Per-viewer state record keyed by session ID, persisted by `SessionManager` |
| **RetryPolicy** | Value type expressing HTTP retry semantics (attempts, backoff, per-attempt timeout) |

## Architecture decision records

Significant design decisions are recorded in [docs/adr/](adr/):

- [ADR 0001](adr/0001-ad-conditioning-sidecar-vs-embedded.md) — Ad conditioning: sidecar service vs embedded transcoding
- [ADR 0002](adr/0002-drm-passthrough-vs-termination.md) — DRM: passthrough vs termination
- [ADR 0003](adr/0003-scte35-binary-library-choice.md) — SCTE-35 binary library choice

## Documentation coverage

| Area | Status |
|------|--------|
| Architecture overview | [docs/architecture.md](architecture.md) |
| `ad` module | [docs/modules/ad.md](modules/ad.md) |
| `hls` module | [docs/modules/hls.md](modules/hls.md) |
| `dash` module | [docs/modules/dash.md](modules/dash.md) |
| `server` module | [docs/modules/server.md](modules/server.md) |
| `session` module | [docs/modules/session.md](modules/session.md) |
| `http_retry` module | [docs/modules/http_retry.md](modules/http_retry.md) |
| `cache` module | [docs/modules/cache.md](modules/cache.md) |
| API reference | [docs/api/README.md](api/README.md) |
| Configuration reference | [README.md § Configuration](../README.md#configuration) |
| Metrics reference | [README.md § Metrics](../README.md#metrics) |
| Performance benchmarks | [BENCHMARK.md](../BENCHMARK.md) |

<!-- TODO: document src/ad/vast/ parser internals -->
<!-- TODO: document src/server/url_validation.rs SSRF rules -->
