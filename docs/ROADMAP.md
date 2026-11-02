# Roadmap

The riskiest work comes first. M1 is a set of spikes that answer the biggest unknowns before any
features are built on top of them.

## M0 — Foundations

- [x] Stack decided, repository initialized, planning docs written
- [ ] Dev environment on this PC (see [DEVELOPMENT.md](DEVELOPMENT.md))
- [ ] Target VM hardware check: `vainfo`, CPU, cores/RAM given to the VM
- [ ] Pick the base distro/GStreamer version, the same for dev (WSL) and the production image

## M1 — Spikes (throwaway code is fine)

- **S1 Twitch playback:** fetch a playback token and master playlist for a channel, follow the
  media playlist for a while, and log the ad markers. Record playlists from real ad breaks as test fixtures.
- **S2 Proxy:** minimal axum proxy that rewrites playlists, plus a plain HTML page with hls.js
  that plays it. Test what dropped ad segments do to the player (stall, resume, jump back to live)
  and whether segments can come straight from the Twitch CDN (CORS).
- **S3 Composite:** GStreamer 2×2 from 4 live channels → HLS with software encoding, measuring CPU.
  Then the same with VA-API on the target VM.
- **S4 Dynamic changes:** while S3 runs, move and resize tiles, replace one input and kill one input
  (its slate must appear), all without the output stopping.

**Done when:** we know the CPU/GPU cost of one multiview session and the ad-break behaviour is
documented, with fixtures.

## M2 — Server core & desktop basics

- Cargo workspace (crates as in [ARCHITECTURE.md](ARCHITECTURE.md#server-server-rust)), env config,
  `tracing` logs, SQLite store
- Device code login, token refresh, logout
- Helix: followed live streams, search, channel info
- Single view via the proxy (S2, brought to production quality)
- Web: app shell, routing, Login (code + QR), Home, Search, Player
- Dockerfile + compose that run on the target VM

**Needs from you:** a Twitch app registration (Client ID + secret).
**Done when:** on desktop you can log in via phone, see followed live channels, search, and watch
one stream served by the container on the VM.

## M3 — Ad handling v1

- Ad detection module with fixture tests
- Strategy chain: backup playlist swap, then drop + notify
- Client overlay for ad breaks
- Log which strategy worked for each break

**Done when:** an ad break in single view shows either backup content or a clean overlay, and
playback resumes on its own.

## M4 — Multiview

- Session API (create / update / delete), layouts as data
- Pipeline manager: one actor per session; add / replace / remove tiles and change layouts at runtime;
  a slate per tile
- Per-tile audio renditions + master playlist; audio switching on the client
- Encoder/decoder probing (VA-API → software), diagnostics endpoint
- Session limits, idle teardown
- Web: multiview builder, tile focus (audio, later chat), swap/remove tiles

**Done when:** 2×2 runs stable for an hour on the target; layout and tile changes don't interrupt
the output; audio switching is instant.

## M5 — Android TV

- 10-foot UI pass: focus styles, spatial navigation on every screen, Back key handling
- Kotlin shell: WebView, Leanback launcher + banner, key mapping, keep screen on, cleartext LAN only
- Server discovery: mDNS + manual entry
- Search with the system on-screen keyboard
- Testing: Android TV emulator + a real device

**Needs from you:** which Android TV device(s) you use.
**Done when:** the whole app is usable with nothing but the TV remote.

## M6 — Chat (read-only)

- Anonymous IRC reader on the server, fanned out over the WebSocket
- Chat panel (desktop: side panel, TV: overlay) for the focused stream, with Twitch emotes and badges

**Done when:** chat follows focus in both single view and multiview.

## M7 — Polish & operations

- Settings screen: quality, ad strategy order, output resolution, access PIN
- Diagnostics screen
- Install guide and example compose file
- GitHub repository + CI (build, tests, image)

## Backlog (after v1)

- Profiles per person
- Sending chat (desktop)
- Third-party emotes (7TV, BTTV, FFZ)
- Go-live notifications (EventSub)
- VODs and clips
- Lower-latency output (LL-HLS, or WebRTC via `webrtcsink`)
- Sharing one multiview session across devices
- Ad-free playback for Turbo / subscribed accounts
- Native ExoPlayer surface on the TV, if WebView playback is too slow
- Remote access (e.g. via Tailscale)

## Open questions

- **Does VirGL in the target VM expose VA-API decode/encode?** If not, the options are:
  - run the container on the host, or in an LXC container with `/dev/dri`
  - pass the iGPU through with VFIO
  - stay with software encoding, which is probably enough for one 2×2 session (measured in S3)
- Which Android TV device(s)?
- Single view segments: proxy them through the server, or point clients directly at the Twitch CDN? → S2

## Risks

| Risk | Impact | Mitigation |
|---|---|---|
| Twitch changes ad insertion or the playback token API | Ad handling or playback breaks | Isolated modules, fixture tests, quick updates |
| Avoiding ads is against Twitch's ToS | Twitch may block techniques | Anonymous playback tokens (the account is not involved); drop + overlay always works |
| No hardware video support in the VM | High CPU use, fewer sessions | Software path treated as first-class; measured early (S3) |
| hlsdemux2 behaviour on stalls and discontinuities | Frozen tiles | `fallbackswitch` + slate, tested in S4; plan B: our own `appsrc` input |
| WebView playback on weak TV hardware | Stutter | Only one stream ever (the server merges); ExoPlayer fallback |
