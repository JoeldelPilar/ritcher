# Module: `dash`

Source: `src/dash/`

Handles DASH MPD parsing, SCTE-35 EventStream detection, SSAI period-based ad insertion, and SGAI callback EventStream injection.

## Submodules

| File | Responsibility |
|------|---------------|
| `parser.rs` | Parse and serialize DASH MPDs via `dash-mpd` |
| `cue.rs` | Detect SCTE-35 EventStream elements → `Vec<DashAdBreak>` |
| `interleaver.rs` | SSAI: insert ad Periods into the MPD |
| `sgai.rs` | SGAI: inject callback EventStream elements |

## Parser

Uses `dash-mpd 0.17`. Fetches the origin MPD body (via `ManifestCache` + `RetryPolicy`), deserializes to the `dash_mpd::MPD` type, applies transformations, then re-serializes to bytes.

**BaseURL resolution**: The parser resolves BaseURL elements at all MPD hierarchy levels (MPD → Period → AdaptationSet → Representation) before rewriting segment URLs through the proxy. Relative URLs are resolved against the outermost BaseURL that applies.

**SegmentTemplate rewriting**: `media` and `initialization` attribute URIs in `SegmentTemplate` elements are rewritten to `/stitch/{session_id}/segment/{path}?origin={base}`.

## SCTE-35 detection (`cue.rs`)

Scans all `EventStream` elements in each Period for the `urn:scte:scte35:2013:xml` scheme URI. Each matching EventStream signals an ad break. Returns `Vec<DashAdBreak>`, each holding the Period index and break duration.

## SSAI — Period-based ad insertion (`interleaver.rs`)

For each `DashAdBreak`, inserts a new DASH Period immediately after the signaling Period. The new Period contains:
- A `SegmentList` referencing the ad segments (proxied through `/stitch/{session_id}/ad/{name}`).
- Mirrored AdaptationSets from the content Period (preserving bandwidth and language attributes).

Both video and audio AdaptationSets are mirrored so the player receives a complete, playable ad Period.

## SGAI — Callback EventStreams (`sgai.rs`)

Injects a `urn:mpeg:dash:event:callback:2015` EventStream element into the MPD Period that carries the SCTE-35 signal. The callback URL points at the asset-list endpoint:

```
https://stitcher.example.com/stitch/{id}/asset-list/{break_id}?dur={seconds}
```

The SCTE-35 EventStream is stripped after injection to prevent double-signaling. The asset-list endpoint is shared with HLS SGAI — the Player (dash.js, Shaka) fetches ad creatives from the same JSON response.
