# Ritcher Domain Context

Ritcher is a server-side HLS/DASH ad-insertion stitcher. It sits between an Origin CDN and a video Player, rewriting manifests to splice ads into live and VOD streams via either SSAI or SGAI.

## Language

### Roles

**Stitcher**:
The Ritcher server itself — the process that fetches origin manifests, plans ad placement, and serves rewritten playlists/manifests to players.
_Avoid_: proxy, mediator (the term "stitching pipeline" refers to the internal flow, not the product).

**Origin**:
The upstream CDN or packager that serves the unmodified source HLS/DASH stream Ritcher pulls from.
_Avoid_: source, upstream, backend.

**Player**:
The client (hls.js, Shaka, dash.js, AVPlayer, ExoPlayer) that consumes Ritcher's stitched output.
_Avoid_: client, viewer (a viewer is a person; a player is software).

### Insertion modes

**SSAI** (Server-Side Ad Insertion):
Mode where Ritcher splices `AdSegment`s directly into the manifest so the player sees one continuous stream.
_Avoid_: server-stitched ads, in-stream ads.

**SGAI** (Server-Guided Ad Insertion):
Mode where Ritcher injects markers (`EXT-X-DATERANGE` for HLS Interstitials, callback `EventStream` for DASH) and the player fetches ad creatives separately via an asset list.
_Avoid_: client-side ad insertion, CSAI (CSAI implies the player owns ad selection — in SGAI the server still decides).

### Streaming primitives

**Manifest**:
Umbrella term for the text document describing a stream — covers both DASH MPDs and HLS playlists.
_Avoid_: index, descriptor.

**Playlist**:
The HLS-specific form of a manifest (`.m3u8`). A master playlist references one or more media playlists.
_Avoid_: HLS manifest (use Playlist when HLS-specific).

**MPD** (Media Presentation Description):
The DASH-specific form of a manifest (`.mpd`).
_Avoid_: DASH playlist.

**Period**:
A DASH MPD time range with its own AdaptationSets. Ad insertion in DASH happens by adding new Periods.

**EventStream**:
A DASH MPD element carrying timed signals. Ritcher reads SCTE-35 `EventStream`s for ad-break detection and writes callback `EventStream`s for SGAI.

**Segment**:
A single media chunk (TS or CMAF) referenced by a manifest.

**LL-HLS** (Low-Latency HLS):
HLS profile using partial segments and blocking playlist reload to drive sub-3s latency. Ritcher passes these tags through with rewritten URIs.

**Interstitial**:
An HLS Interstitial — an ad break expressed as `EXT-X-DATERANGE` with `CLASS="com.apple.hls.interstitial"` per RFC 8216bis. Resolved by the player against an asset list.
_Avoid_: HLS ad slot, daterange ad.

### Ad-break signaling

**SCTE-35**:
The industry standard for ad-break signaling. Ritcher reads it from HLS CUE tags and from DASH EventStream elements.

**CUE marker**:
An HLS tag (`EXT-X-CUE-OUT`, `EXT-X-CUE-IN`, `EXT-X-CUE-OUT-CONT`) carrying SCTE-35 ad-break boundaries.
_Avoid_: cue point, ad cue.

**Ad break**:
A time window in the source stream signaled for ad insertion. Modeled as `AdBreak` (HLS, segment-index based) or `DashAdBreak` (DASH, period-index based).
_Avoid_: ad pod (ad pod is a VAST concept — a sequence of ads filling a break, not the break itself).

**EXT-X-DATERANGE**:
The HLS tag Ritcher writes in SGAI mode to mark an Interstitial. Carries the asset-list URL and timing.

### Ad payload

**Ad creative**:
A complete ad as a logical unit — a full HLS playlist URL or MP4 URL served in the asset-list `ASSETS` array. SGAI delivers ad creatives. Modeled as `AdCreative`.
_Avoid_: ad asset (ambiguous with asset list), ad clip.

**Ad segment**:
One TS or CMAF chunk of an ad creative, splice-ready for SSAI mode. Modeled as `AdSegment`.
_Avoid_: ad chunk.

**Ad provider**:
The strategy that produces ads for a given break — implements the `AdProvider` trait. Built-in implementations: `VastAdProvider`, `StaticAdProvider`, `DemoAdProvider`.

**VAST**:
The IAB XML protocol Ritcher speaks to ad servers (2.0/3.0/4.0, including wrapper chains).

**Slate**:
Filler content shown when VAST returns no ads or an ad fetch fails. Provided by `SlateProvider`.
_Avoid_: fallback ad, default content.

**Ad conditioning**:
Pre-flight validation of ad creatives for codec, resolution, and MIME-type compatibility with the source stream. Warning-level — does not block insertion.

**Tracking beacon**:
A URL Ritcher fires server-side on ad-segment delivery to report VAST events (impression, quartiles, complete, error).
_Avoid_: pixel, ping.

**OMID** (Open Measurement Interface Definition):
IAB SDK for ad viewability measurement. Ritcher accumulates OMID `Verification` resources from VAST wrapper chains and surfaces them in `AdCreative` for SGAI delivery.

**Asset list**:
A JSON document at `/stitch/{id}/asset-list/{break_id}` listing ad creatives for one Interstitial. Consumed by HLS Interstitials players and by DASH callback EventStream consumers.

### Session and proxy

**Session**:
A per-viewer state record (`Session` struct) keyed by session ID, holding the origin URL plus timestamps. Stored in `SessionManager`'s memory backend (DashMap) or Valkey backend.
_Avoid_: connection, request context (those mean network-layer state — a Ritcher Session is logical and outlives any single HTTP request).

**Segment proxy**:
The handler at `/stitch/{id}/segment/*` and `/stitch/{id}/ad/{name}` that streams content and ad bytes from origin to player without buffering.

## Proposed (not yet implemented)

Architectural seams under discussion in the current /improve-codebase-architecture session. None of these exist in `src/` yet.

**SsaiAdProvider**:
Proposed split of `AdProvider` — the SSAI-only half returning `AdSegment`s.

**SgaiAdProvider**:
Proposed split of `AdProvider` — the SGAI-only half returning `AdCreative`s.

**ValidatedOrigin**:
Proposed Axum extractor representing an origin URL that has passed SSRF, redirect, and DNS-rebind checks at the type level.

**ValidatedSessionId**:
Proposed Axum extractor representing a session ID that has passed character-set validation at the type level.

**SessionStore**:
Proposed trait abstracting session persistence so memory and Valkey backends become adapter implementations rather than enum variants of `SessionManager`.

**InterstitialPlanner**:
Proposed module owning HLS SGAI writing — PDT synthesis, `EXT-X-DATERANGE` emission, and asset-list URL composition.

**ManifestStore**:
Proposed trait wrapping origin manifest caching, with `TtlMemory` and `NoCache` adapters replacing the concrete `ManifestCache`.

**RetryPolicy**:
Proposed value type replacing the current `RetryConfig` struct, expressing HTTP retry semantics as a first-class domain concept rather than a config bag.

## Relationships

- A **Stitcher** serves many **Players** and pulls from one or more **Origins**.
- A **Player** opens one **Session**; a **Session** maps to exactly one **Origin** URL.
- A **Manifest** is either a **Playlist** (HLS) or an **MPD** (DASH).
- An **MPD** contains one or more **Periods**; a **Period** may contain **EventStreams**.
- A **Playlist** contains **Segments** and may contain **CUE markers**.
- An **Ad break** is detected from **SCTE-35** carried in **CUE markers** (HLS) or **EventStreams** (DASH).
- An **Ad break** is filled by either many **Ad segments** (SSAI) or many **Ad creatives** (SGAI).
- An **Ad creative** is composed of one or more **Ad segments** at the media layer.
- An **Ad provider** produces **Ad segments** and **Ad creatives** for a given **Ad break** and **Session**.
- **VAST** is the wire protocol an **Ad provider** may use; **OMID** verifications travel inside VAST responses and surface on **Ad creatives**.
- **Tracking beacons** belong to **Ad segments** (via `AdTrackingInfo`) and fire when the **Segment proxy** serves them.
- An **Asset list** belongs to one **Ad break** and lists its **Ad creatives**.
- **Slate** substitutes for **Ad segments** or **Ad creatives** when an **Ad provider** returns nothing.

## Example dialogue

> **Dev:** "When the **Player** hits a **CUE marker** in the **Playlist**, does Ritcher splice **Ad segments** inline or just point at an **Asset list**?"
>
> **Domain expert:** "Depends on the mode. In **SSAI** the **Stitcher** rewrites the **Playlist** so the **Ad segments** appear in the segment sequence and the **Player** sees one continuous stream. In **SGAI** the **Stitcher** writes an `EXT-X-DATERANGE` **Interstitial** instead, and the **Player** fetches the **Asset list** to get the **Ad creatives**."
>
> **Dev:** "And the **Tracking beacons** — who fires those?"
>
> **Domain expert:** "Ritcher does, server-side, when the **Segment proxy** delivers an **Ad segment**. The beacon URLs come from **VAST** and ride along on the `AdSegment` via `AdTrackingInfo`. **OMID** verifications are different — those go to the **Player** through the **Asset list** because viewability has to run client-side."
>
> **Dev:** "What if the **Ad provider** returns nothing?"
>
> **Domain expert:** "Then **Slate** fills the **Ad break**. The **Player** never sees a hole."

## Flagged ambiguities

- **Session** was overloaded between Ritcher's per-viewer logical record and HTTP/network session state. Resolved: in this codebase **Session** always means the per-viewer record persisted by `SessionManager`. Network-layer state has no domain term — refer to it as "HTTP request" or "TCP connection" explicitly.
- **Manifest** vs **Playlist**: **Manifest** is the umbrella term covering both HLS and DASH. **Playlist** is HLS-specific (`.m3u8`); **MPD** is DASH-specific (`.mpd`). Use the specific term when the format matters; use **Manifest** only when speaking generically.
- **Ad creative** vs **Ad segment**: an **Ad creative** is one full ad as a logical unit (one `AdCreative`, served via the **Asset list** in SGAI). An **Ad segment** is one media chunk of that ad (one `AdSegment`, spliced inline in SSAI). One creative decomposes into many segments at the media layer.
- **Stitcher** vs the internal stitching pipeline: capital-S **Stitcher** is the Ritcher product/server. The code path that fetches, parses, plans, and rewrites is the "stitching pipeline" (lowercase) — an internal implementation concept, not a domain term.
- **Ad break** vs **ad pod**: an **Ad break** is the time window signaled in the source stream (a Ritcher concept, modeled as `AdBreak` / `DashAdBreak`). An "ad pod" is a VAST concept — the sequence of ads chosen to fill a break. Ritcher code uses **Ad break** consistently; reserve "pod" for VAST-protocol discussions.
