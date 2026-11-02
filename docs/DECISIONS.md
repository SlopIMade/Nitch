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
