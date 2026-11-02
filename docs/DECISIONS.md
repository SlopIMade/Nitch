# Decision log

Newest entries at the bottom. Each entry records what was decided, why, and what was rejected.
If a decision changes, add a new entry instead of rewriting the old one.

### D1 — Server-side merge for multiview, passthrough for single view
*2026-11-02*

TV sticks can usually hardware-decode only one or two videos at a time, and one merged stream works
on every client. Single view doesn't need re-encoding, so it gets none.
**Rejected:** client-side multiview with several `<video>` elements (fails on TV hardware).

### D2 — Server in Rust with GStreamer
*2026-11-02*

The multiview pipeline has to change while it runs (layouts, tiles, ad breaks, offline streams)
without restarting. GStreamer's `compositor` + `fallbackswitch` are built for this. `gstreamer-rs` is
the best-supported binding, and elements like `fallbackswitch`, `hlscmafsink` and `webrtcsink` are Rust
crates we can link statically. HTTP via tokio + axum.
**Rejected:**
- Go + FFmpeg: the filter graph is fixed once started, so changes need a restart and one stalled input stalls the whole output.
- Node + FFmpeg: same limits, and weaker for media work that runs for hours.

### D3 — Web client in React + TypeScript + Vite
*2026-11-02*

Norigin Spatial Navigation is a proven library for TV remote focus handling, and React has the largest
ecosystem. hls.js for playback, TanStack Query for data.
**Rejected:** SolidJS and Svelte 5 (no mature spatial navigation library).

### D4 — Android TV app is a thin Kotlin WebView shell
*2026-11-02*

One UI for desktop and TV. UI updates ship with the server, and the APK only handles discovery, key
mapping and a small JS bridge.
**Rejected:**
- Capacitor: bundles the UI and adds moving parts for little gain.
- A native Compose for TV app: a second UI to maintain.

### D5 — One shared Twitch account, device code login
*2026-11-02*

Built for household use. Logging in on the TV via phone uses Twitch's official device code flow. The
server stores the tokens; clients never see them.
**Later:** profiles per person (backlog).

### D6 — Read-only chat in v1, via anonymous IRC
*2026-11-02*

Cheap to build and needs no extra scopes. It sits behind a trait so EventSub can replace it.

### D7 — Multiview output is HLS (CMAF) with one audio rendition per tile
*2026-11-02*

Works with hls.js on desktop and in the TV WebView, and switching audio happens instantly on the client.
**Later:** LL-HLS or WebRTC, if latency matters.

### D8 — Pluggable encoders, with software encoding as a first-class path
*2026-11-02*

The target is a VM with an AMD iGPU passed through via QEMU VirGL, and hardware video support there
is unverified. VA-API is used when available, x264 otherwise.

### D9 — Monorepo, local git only for now
*2026-11-02*

`server/`, `web/`, `android-tv/`, `deploy/`, `docs/`. GitHub comes later.

### D10 — Development on Windows through WSL2
*2026-11-02*

The Linux parts (Rust, GStreamer, Docker Engine) run in WSL2 Ubuntu; web and Android tooling run on
Windows. We use Docker Engine inside WSL rather than Docker Desktop: it is lighter and has no licence questions.

### D11 — Base: Ubuntu 24.04 LTS, GStreamer 1.24
*2026-11-02*

Dev (WSL) and the production image use the same release, so GStreamer versions match. Verified:
gstreamer-rs 0.25 and gst-plugins-rs 0.15 (`fallbackswitch`, `fallbacksrc`, `hlssink3`, `hlscmafsink`)
build, register statically and run against GStreamer 1.24.2.
**Revisit** if we hit a bug that is only fixed in a newer GStreamer.

### D12 — Chat is a client-side overlay: off, focused or combined
*2026-11-02*

The chat is drawn as HTML over the video, so it stays sharp, changes instantly and costs the server
nothing. Combined mode is **one merged window**, with each line tagged by its channel; you choose which
streams are included. It floats over the video with a slightly transparent background, and size,
position and font size are adjustable. Settings are saved per device, because TV and desktop screens
differ. Extends D6.
**Rejected:**
- Burning chat into the server's video: blurry, delayed, and the same for every device.
- One chat window per stream: chosen against for v1.

### D13 — Free tile layouts in v1
*2026-11-02*

Tiles can be moved and resized freely; presets are only starting points. A layout is a list of normalized
rectangles, applied live by the compositor. The video reaches the client a few seconds after the
server composites it, so the editor previews changes with outlines, and markers in the playlist
keep client overlays (focus frame, labels) in sync with the picture.
**Rejected:** presets only, with free resizing after v1.

### D14 — Persistent player; settings open over it
*2026-11-02*

The player is mounted at the app root and never unmounts on navigation. Settings, search and menus are
layers on top of it, so changing a setting never stops a stream.

### D15 — Ad gaps are always filled, never just dropped
*2026-11-02*

In S2, dropping ad segments froze the picture for *(gap − buffer)* seconds and left the player
stuttering at the live edge. Ad segments are therefore replaced 1:1:
1. by backup segments with the same `PROGRAM-DATE-TIME` (byte-identical in S2), or
2. by pre-encoded slate segments.

The output timeline stays continuous.
**Rejected:** drop + client overlay only.

### D16 — Single-view segments load directly from the Twitch CDN
*2026-11-02*

The CDN sends `access-control-allow-origin: *` (S2), so clients fetch the live segments themselves
and the server only rewrites playlists. Proxying segments stays available as a fallback setting.
Slate segments come from our server.

### D17 — Fixed 60×60 grid; multiview sessions stored on the server
*2026-11-02*

The multiview canvas is a fixed 16:9 picture divided into 60 × 60 cells. Tiles are whole-cell
rectangles plus z-order. Sessions (a name + tiles) are stored on the server, with no limit on their
number, and every device sees the same sessions with the same layout. A different screen aspect ratio
only adds black bars, because the layout lives in the server's video. Two devices on one session share
its pipeline.
**Why 60 and not 50 (the user's first idea):**
- at 1080p a cell is exactly 32 × 18 px (with 50: 38.4 × 21.6)
- 60 divides into halves, thirds, quarters, fifths and sixths
- equal width and height in cells is exactly 16:9

Refines D13: free placement now means grid-aligned placement, and "own presets" become sessions.
