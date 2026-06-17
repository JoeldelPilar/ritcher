# Module: `ad`

Source: `src/ad/`

Owns ad decision logic: the `AdProvider` abstraction, three built-in implementations, VAST protocol handling, tracking beacon delivery, slate fallback, and ad conditioning.

## Core types

### `AdSegment`

One TS or CMAF chunk of an ad creative. Used in SSAI mode — the stitcher splices `AdSegment`s directly into the Playlist.

```
AdSegment {
    uri: String                         — proxied URI through /stitch/{id}/ad/{name}
    duration: f32                       — segment duration in seconds
    tracking: Option<AdTrackingInfo>    — present only for VAST-sourced ads
}
```

### `AdCreative`

A complete ad unit for SGAI mode. One entry in the asset-list JSON `ASSETS` array.

```
AdCreative {
    uri: String                     — HLS playlist URL or MP4 URL
    duration: f64                   — creative duration in seconds
    verifications: Vec<Verification> — OMID verification resources from VAST
}
```

### `AdTrackingInfo`

Tracking metadata attached to an `AdSegment`. The segment handler reads this to fire VAST tracking beacons at the right quartile.

```
AdTrackingInfo {
    impression_urls: Vec<String>
    tracking_events: Vec<TrackingEvent>  — quartiles: start, firstQuartile, midpoint, thirdQuartile, complete
    error_url: Option<String>
    total_segments: usize               — denominator for quartile calculation
    segment_index: usize                — numerator
}
```

## `AdProvider` trait

```rust
#[async_trait]
trait AdProvider: Send + Sync {
    async fn get_ad_segments(&self, duration: f32, session_id: &str) -> Vec<AdSegment>;
    fn resolve_segment_url(&self, ad_name: &str, session_id: &str) -> Option<String>;
    fn resolve_segment_with_tracking(&self, ad_name: &str, session_id: &str) -> Option<ResolvedSegment>;
    fn cleanup_cache(&self) {}
    async fn get_ad_creatives(&self, duration: f32, session_id: &str) -> Vec<AdCreative>;
}
```

- `get_ad_segments` — called by the HLS/DASH stitching pipeline to fill an ad break of the given duration.
- `resolve_segment_url` / `resolve_segment_with_tracking` — called by the ad segment handler to map a proxied name (e.g. `break-0-seg-3.ts`) back to the source URL.
- `get_ad_creatives` — called by the asset-list handler for SGAI. The default impl adapts `get_ad_segments`; `VastAdProvider` overrides it for proper creative-level URLs.
- `cleanup_cache` — called every 60s by the background task; stateless providers (Static, Demo) use the default no-op.

## Built-in providers

### `StaticAdProvider`

Serves a pre-configured ad segment source URL. Cycles through 10 segments (`out_000.ts` … `out_009.ts`). Used in dev/testing when no VAST endpoint is available.

Segment naming: `break-{break_idx}-seg-{seg_idx}.ts`

### `DemoAdProvider`

Five visually distinct creatives mapped by break index. Each break index routes to a different creative directory (`creative-1` through `creative-5`, cycling). Used in customer demos to show varied ad content.

Segment naming:
- HLS MPEG-TS: `break-{break}-seg-{seg}.ts`
- DASH fMP4 video init: `break-{break}-vinit.m4s`
- DASH fMP4 audio init: `break-{break}-ainit.m4s`
- DASH fMP4 video segment: `break-{break}-vseg-{seg}.m4s`
- DASH fMP4 audio segment: `break-{break}-aseg-{seg}.m4s`

DASH ad segments are sourced from the DASH-IF Big Buck Bunny CDN (`dash.akamaized.net`).

### `VastAdProvider`

Fetches and parses VAST 2.0/3.0/4.0 XML from an ad server. Supports wrapper chains (resolved recursively). Caches responses per ad break (keyed by `session_id + duration`) with a TTL and size bound, enforced by `cleanup_cache`.

Source: `src/ad/vast_provider/`

**Fetch flow:**
1. Build VAST request URL (substitutes `[DURATION]` and `[CACHEBUSTING]` macros).
2. Execute HTTP GET via `RetryPolicy::production().with_timeout(...)`.
3. Parse VAST XML → resolve wrapper chain (recursive fetches, depth-limited).
4. Extract `MediaFile` URLs, tracking events, OMID verification resources.
5. Cache result; return `Vec<AdSegment>` or `Vec<AdCreative>`.

If VAST returns no ads and a `SlateProvider` is configured, slate segments fill the break instead.

## VAST parser

Source: `src/ad/vast/`

Parses VAST XML using `quick-xml`. Supports:
- `<InLine>` and `<Wrapper>` ads
- `<MediaFile>` (progressive and streaming)
- `<TrackingEvents>` (impression, quartiles, error)
- `<Verification>` (OMID resources)

Types: `src/ad/vast/types.rs` — `VastAd`, `MediaFile`, `TrackingEvent`, `Verification`, etc.

## Ad conditioning

Source: `src/ad/conditioning.rs`

Warning-level pre-flight check on ad creative compatibility with the source stream. Inspects VAST `MediaFile` MIME type, resolution, and codec. Emits `tracing::warn!` on mismatch but does not block insertion.

See [ADR 0001](../adr/0001-ad-conditioning-sidecar-vs-embedded.md) for the architectural decision on why transcoding lives in a sidecar, not in Ritcher.

## Tracking beacons

Source: `src/ad/tracking.rs`

Fired server-side by the segment handler when an `AdSegment` with `tracking: Some(…)` is served. Events:
- `impression` — fires on the first segment of a creative
- `start`, `firstQuartile`, `midpoint`, `thirdQuartile`, `complete` — fires at segment-index–based quartile boundaries

Beacon fires are fire-and-forget (Tokio spawn); failures are logged but do not affect segment delivery. Results recorded as `ritcher_tracking_beacons_total`.

## Slate

Source: `src/ad/slate.rs`

`SlateProvider` returns filler `AdSegment`s when `VastAdProvider` receives an empty ad response. Configured via `SLATE_URL` and `SLATE_SEGMENT_DURATION`. If no slate is configured, an empty `Vec` is returned, which the stitcher treats as a pass-through (no ad break inserted).
