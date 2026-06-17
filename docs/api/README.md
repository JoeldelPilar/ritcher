# API Reference

Base URL: configured via `BASE_URL` (e.g. `https://stitcher.example.com`).

All stitcher endpoints require a `{session_id}` path segment — an arbitrary string chosen by the caller (or generated per viewer). Session IDs are validated for character-set safety.

---

## Utility endpoints

### `GET /health`

JSON health check.

**Response** `200 application/json`:
```json
{
  "status": "ok",
  "version": "0.1.0",
  "active_sessions": 42,
  "uptime_seconds": 3600
}
```

Used by Docker `HEALTHCHECK` and load balancer probes.

---

### `GET /metrics`

Prometheus metrics in text exposition format.

**Response** `200 text/plain`

See [README.md § Metrics](../../README.md#metrics) for the full metric list.

---

## Demo endpoints

Demo endpoints serve synthetic manifests with built-in ad-break signals. No external origin or ad server required. Useful for smoke testing and player integration.

### `GET /demo/playlist.m3u8`

HLS media playlist with `EXT-X-CUE-OUT/IN` markers and real Mux test segments.

### `GET /demo/ll-hls/playlist.m3u8`

LL-HLS variant with `EXT-X-PART` partial segments, `EXT-X-SERVER-CONTROL`, and `EXT-X-CUE-OUT/IN` markers.

### `GET /demo/manifest.mpd`

DASH MPD with a SCTE-35 `EventStream` element signaling an ad break.

---

## Stitcher endpoints

### `GET /stitch/{session_id}/playlist.m3u8`

Stitched HLS playlist with ad insertion applied.

**Query parameters:**

| Parameter | Required | Description |
|-----------|----------|-------------|
| `origin` | Yes (first request) | URL of the origin HLS playlist. Stored in the session and reused on subsequent requests. |

**Response** `200 application/vnd.apple.mpegurl`

The origin playlist is fetched, parsed, and returned with one of:
- **SSAI**: ad segments interleaved in place of content segments at each `AdBreak`.
- **SGAI**: `EXT-X-DATERANGE` Interstitial tags injected at each `AdBreak`.

All segment and sub-manifest URIs in the response are rewritten through the stitcher proxy.

**Errors:**
- `400` — invalid `session_id` or `origin` URL (SSRF check failed).
- `422` — origin returned a body that could not be parsed as an HLS playlist.
- `502` — origin fetch failed or response body exceeded 10 MB.

---

### `GET /stitch/{session_id}/manifest.mpd`

Stitched DASH MPD with ad insertion applied.

**Query parameters:**

| Parameter | Required | Description |
|-----------|----------|-------------|
| `origin` | Yes (first request) | URL of the origin DASH MPD. |

**Response** `200 application/dash+xml`

Same pipeline as the HLS handler but for DASH:
- **SSAI**: ad Periods inserted after SCTE-35 EventStream Periods.
- **SGAI**: `urn:mpeg:dash:event:callback:2015` EventStream injected; SCTE-35 EventStream stripped.

**Errors:** same as playlist handler, with `422` for MPD parse failures.

---

### `GET /stitch/{session_id}/segment/{*segment_path}`

Proxy a content segment from the origin CDN to the player.

**Query parameters:**

| Parameter | Required | Description |
|-----------|----------|-------------|
| `origin` | Yes | Base URL of the origin CDN. The full segment URL is `{origin}/{segment_path}`. |

**Response** `200` — segment bytes streamed directly from the origin with the origin's `Content-Type`.

The segment is not buffered in Ritcher memory; the response body is piped as a stream. `RetryPolicy` applies (2 attempts, 500ms backoff).

---

### `GET /stitch/{session_id}/ad/{ad_name}`

Proxy an ad segment to the player and fire VAST tracking beacons.

**Path segment `{ad_name}`** encodes the break and segment index. Format depends on provider:

| Provider | HLS segment | DASH video | DASH audio |
|----------|------------|-----------|-----------|
| Static | `break-{b}-seg-{s}.ts` | — | — |
| Demo | `break-{b}-seg-{s}.ts` | `break-{b}-vseg-{s}.m4s` | `break-{b}-aseg-{s}.m4s` |
| VAST | `break-{b}-seg-{s}.ts` | — | — |

The handler resolves `{ad_name}` to a source URL via `AdProvider::resolve_segment_with_tracking`, fetches the bytes, fires any pending VAST tracking beacons (impression, quartiles), and streams the bytes to the player.

**Response** `200` — ad segment bytes with appropriate `Content-Type`.

**Errors:**
- `404` — `{ad_name}` could not be resolved (unknown format or stale session).
- `502` — upstream segment fetch failed.

---

### `GET /stitch/{session_id}/asset-list/{break_id}`

Asset-list JSON for SGAI. Consumed by HLS Interstitials players and DASH callback EventStream consumers.

**Path segment `{break_id}`** matches the `ID` attribute in the corresponding `EXT-X-DATERANGE` tag (HLS) or the event ID in the callback EventStream (DASH). Format: `break-{n}` (e.g. `break-0`, `break-1`).

**Query parameters:**

| Parameter | Required | Description |
|-----------|----------|-------------|
| `dur` | Yes | Ad break duration in seconds. Passed to `AdProvider::get_ad_creatives`. |

**Response** `200 application/json`:
```json
{
  "ASSETS": [
    {
      "URI": "https://ads.example.com/creative-1/playlist.m3u8",
      "DURATION": 30.0
    }
  ]
}
```

Each object in `ASSETS` is one `AdCreative`. For VAST-sourced ads the `URI` is the HLS playlist URL from the VAST `MediaFile` element. For static/demo providers it is the proxied ad source URL.

**Errors:**
- `400` — missing `dur` parameter or invalid `break_id`.
- `500` — ad provider error.

---

## Response headers

All responses include:

| Header | Value |
|--------|-------|
| `X-Ritcher-Version` | Semver from `Cargo.toml` |

CORS headers are added for all origins on GET/HEAD/OPTIONS requests (required for browser-based players).
