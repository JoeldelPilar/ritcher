# Module: `server`

Source: `src/server/`

Axum HTTP server: router construction, handler implementations, middleware stack, and shared application state.

## Router

Two construction functions:

- `build_router(config)` — used by E2E tests; no Prometheus recorder.
- `build_router_with_token(config, cancel)` — used by `server::start`; spawns background tasks.

Routes:

| Pattern | Handler |
|---------|---------|
| `GET /` | `health::health_check` |
| `GET /health` | `health::health_check` |
| `GET /metrics` | `metrics::serve_metrics` (production only) |
| `GET /demo/playlist.m3u8` | `demo::serve_demo_playlist` |
| `GET /demo/ll-hls/playlist.m3u8` | `demo::serve_demo_ll_hls_playlist` |
| `GET /demo/manifest.mpd` | `demo::serve_demo_manifest` |
| `GET /dev` | `dev::serve_dev_ui` (404 in production) |
| `GET /stitch/{session_id}/playlist.m3u8` | `playlist::serve_playlist` |
| `GET /stitch/{session_id}/manifest.mpd` | `manifest::serve_manifest` |
| `GET /stitch/{session_id}/segment/{*segment_path}` | `segment::serve_segment` |
| `GET /stitch/{session_id}/ad/{ad_name}` | `ad::serve_ad` |
| `GET /stitch/{session_id}/asset-list/{break_id}` | `asset_list::serve_asset_list` |

## Middleware stack (outermost → innermost)

1. **CORS** — allows GET/HEAD/OPTIONS from any origin. Required RANGE header is allowed for byte-range segment requests. `max_age` 3600s avoids repeated preflight.
2. **Rate limiter** — per-IP token bucket, applied before handlers. Enabled only when `RATE_LIMIT_RPM > 0`.
3. **Version header** — adds `X-Ritcher-Version: {semver}` to every response.

## Handlers

### `playlist` and `manifest`

The core stitching handlers. Both follow the same pipeline:

1. Extract and validate the `?origin=` query param via `url_validation::validate_origin_url`.
2. Upsert a `Session` for the `{session_id}` path segment.
3. Fetch the origin manifest (with `ManifestCache` deduplication and `RetryPolicy` retries).
4. Parse (HLS or DASH).
5. Detect ad breaks (CUE or SCTE-35 EventStream).
6. Branch on `StitchingMode`:
   - **SSAI**: call `AdProvider::get_ad_segments` per break → interleave.
   - **SGAI**: inject markers → asset-list endpoint will be called later by the player.
7. Rewrite all segment/manifest URLs through the proxy.
8. Serialize and return.

### `segment`

Streams content segments from the origin CDN directly to the player without buffering. Uses `RetryPolicy` for the upstream fetch. Does not read segment bytes into memory; pipes the response body as a stream.

### `ad`

Resolves the `{ad_name}` to a source URL via `AdProvider::resolve_segment_with_tracking`, fetches the segment bytes, fires any VAST tracking beacons (server-side), and streams the bytes to the player.

### `asset_list`

Calls `AdProvider::get_ad_creatives(dur, session_id)` and serializes the result as the HLS Interstitials asset-list JSON format:

```json
{"ASSETS": [{"URI": "…", "DURATION": 30.0}]}
```

### `health`

Returns JSON with version, active session count, uptime seconds, and a status string. Used by Docker `HEALTHCHECK` and load balancer probes.

### `demo`

Serves synthetic HLS and DASH manifests with hardcoded SCTE-35 signals and real Mux test segment URLs. Dev-only; does not require an external origin or ad server.

## `AppState`

Shared across all handlers via Axum's `State` extractor. See [architecture.md](../architecture.md#appstate) for the field list.

## SSRF protection

Three layers prevent server-side request forgery:

**`url_validation`** (`src/server/url_validation.rs`): `validate_origin_url` rejects URLs whose resolved host falls in RFC-1918, loopback, link-local, or other non-routable ranges. Called before any fetch to the origin or ad server.

**`SsrfSafeResolver`** (`src/server/dns_resolver.rs`): A custom `hickory-resolver`-based DNS resolver that blocks resolution of private/internal IP ranges. Used by the `reqwest` client in production mode. Prevents DNS rebinding attacks where a public hostname resolves to a private IP.

**Custom redirect policy**: The `reqwest::Client` is built with a redirect policy that calls `validate_origin_url` on every redirect target before following it. Without this, an attacker could point an origin URL at a public host that 3xx-redirects to an internal IP, bypassing the initial validation.

## Rate limiter (`rate_limit.rs`)

Token-bucket implementation keyed by client IP. Reads `X-Forwarded-For` when available, falls back to the TCP peer address. Stale entries (IPs that haven't made a request in a long time) are evicted every 60s by the background task.

## `MAX_MANIFEST_SIZE`

10 MB hard cap on origin response bodies. Prevents OOM from a malicious origin streaming unbounded data via chunked transfer encoding.

Source: `src/server/mod.rs:12`
