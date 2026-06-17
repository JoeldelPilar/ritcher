# Module: `hls`

Source: `src/hls/`

Handles HLS Playlist parsing, CUE marker detection, SSAI ad interleaving, SGAI HLS Interstitials injection, and LL-HLS pass-through.

## Submodules

| File | Responsibility |
|------|---------------|
| `parser.rs` | Parse and serialize HLS playlists via `m3u8-rs` |
| `cue.rs` | Detect `EXT-X-CUE-OUT/IN/CONT` markers → `Vec<AdBreak>` |
| `interleaver.rs` | SSAI: splice `AdSegment`s into the parsed playlist |
| `interstitial.rs` | SGAI: inject `EXT-X-DATERANGE` Interstitials |
| `ll_hls.rs` | LL-HLS: rewrite partial-segment and preload-hint URIs |

## Parser

Uses `m3u8-rs 6.0`. Reads the origin Playlist body (fetched via `ManifestCache` + `RetryPolicy`) and produces an `m3u8_rs::MediaPlaylist` or `MasterPlaylist` value. Serialization back to bytes happens after all transformations are applied.

Master playlist handling: variant-stream `URI` fields are rewritten to route through `/stitch/{session_id}/playlist.m3u8?origin={url}`.

## CUE detection (`cue.rs`)

Scans the media Playlist for:

- `EXT-X-CUE-OUT` — marks the start of an ad break and its duration (seconds).
- `EXT-X-CUE-OUT-CONT` — continuation tag, carries elapsed and total duration.
- `EXT-X-CUE-IN` — marks the end of an ad break.

Returns `Vec<AdBreak>`, each holding the segment index range and duration in seconds. These are consumed by the interleaver (SSAI) or the Interstitial writer (SGAI).

## SSAI interleaver (`interleaver.rs`)

Replaces content segments inside each `AdBreak` window with the `AdSegment`s returned by `AdProvider::get_ad_segments`. Emits:

- `EXT-X-DISCONTINUITY` before the first ad segment.
- `EXT-X-DISCONTINUITY` after the last ad segment (before content resumes).
- Proxied URIs for each ad segment: `/stitch/{session_id}/ad/break-{n}-seg-{m}.ts`.

Content segments inside the break window are dropped from the output Playlist.

## SGAI — HLS Interstitials (`interstitial.rs`)

Instead of splicing media, injects `EXT-X-DATERANGE` tags at each ad break position:

```
#EXT-X-DATERANGE:ID="break-0",CLASS="com.apple.hls.interstitial",
  START-DATE="…",DURATION=30,
  X-ASSET-LIST="https://stitcher.example.com/stitch/{id}/asset-list/0?dur=30"
```

Also:
- **PDT synthesis**: if the origin Playlist lacks `EXT-X-PROGRAM-DATE-TIME`, Ritcher synthesizes one from the current wall clock so the `START-DATE` attribute is meaningful.
- **CUE tag removal**: CUE-OUT/IN/CONT tags are stripped after `EXT-X-DATERANGE` injection to prevent double-signaling.

## LL-HLS pass-through (`ll_hls.rs`)

Low-Latency HLS tags are passed through with URIs rewritten through the proxy:

- `EXT-X-PART` — partial segment URIs rewritten.
- `EXT-X-PRELOAD-HINT` — preload hint URIs rewritten.
- `EXT-X-RENDITION-REPORT` — rendition report URIs rewritten.
- `EXT-X-SERVER-CONTROL` — passed through unchanged.
- `EXT-X-PART-INF` — passed through unchanged.

Blocking playlist reload query strings (`_HLS_msn`, `_HLS_part`) from the Player are forwarded to the origin fetch.
