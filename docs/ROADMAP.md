# Roadmap

The riskiest work comes first. M1 is a set of spikes that answer the biggest unknowns before any
features are built on top of them.

## M0 — Foundations

- [x] Stack decided, repository initialized, planning docs written
- [x] Dev environment on this PC (see [DEVELOPMENT.md](DEVELOPMENT.md)), verified with `scripts/check-dev.sh`
- [ ] Target VM hardware check: `vainfo`, CPU, cores/RAM given to the VM
- [x] Base distro/GStreamer version: Ubuntu 24.04 LTS / GStreamer 1.24.2 for dev and production ([D11](DECISIONS.md#d11--base-ubuntu-2404-lts-gstreamer-124))

## M1 — Spikes (throwaway code is fine)

- [x] **S1 Twitch playback:** fetch a playback token and master playlist for a channel, follow the
  media playlist for a while, and log the ad markers. Record playlists from real ad breaks as test fixtures.
  → [findings](../spikes/s1-twitch-playback/FINDINGS.md): works without login; ad markers appear ~4 s
  early; player types get ads at different times.
- [x] **S2 Proxy:** minimal axum proxy that rewrites playlists, plus a plain HTML page with hls.js
  that plays it. Test what dropped ad segments do to the player (stall, resume, jump back to live)
  and whether segments can come straight from the Twitch CDN (CORS). From S1: check whether the
  `site` and `embed` playlists share one `PROGRAM-DATE-TIME` timeline (segment-exact backup splicing).
  → [findings](../spikes/s2-proxy/FINDINGS.md): live segments of `site` and `embed` are byte-identical;
  the CDN allows CORS; just dropping ads freezes the picture, so gaps must be filled.
- **S3 Composite:** GStreamer 2×2 from 4 live channels → HLS with software encoding, measuring CPU.
  Then the same with VA-API on the target VM.
  - [x] Software part → [findings](../spikes/s3-composite/FINDINGS.md): ~1.75 cores (3900X) for 4×480p
    → 1080p30. One stalled input (e.g. a preroll) can hold up the whole output.
  - [ ] VA-API on the target VM (needs the VM check from M0)
- [x] **S4 Dynamic changes:** while S3 runs, move and resize tiles, replace one input and kill one input
  (its slate must appear), all without the output stopping. From S3:
  - pace every tile and the output by the clock (`fallbacksrc` / `livesync`)
  - give the output audio its own fallback
  - start inputs near the live edge

  → [findings](../spikes/s4-dynamic/FINDINGS.md): all changes run live at 0.98× realtime; the
  dead source shows the slate; 1.85 cores once formats are pinned (unpinned: High 4:4:4 10-bit, unplayable).

**Status:** M1 is done except VA-API on the target VM, which waits for the VM check.

**Done when:** we know the CPU/GPU cost of one multiview session and the ad-break behaviour is
documented, with fixtures.

## M2 — Server core & desktop basics

- [x] Cargo workspace (crates as in [ARCHITECTURE.md](ARCHITECTURE.md#server-server-rust)), env config,
  `tracing` logs, SQLite store
- [x] Device code login, token refresh, logout. Tested end to end with a real account on 2026-11-02.
- [x] Helix: followed live streams, search, channel info
- [x] Single view via the proxy: master rewritten, segments straight from the CDN (D16)
- [x] Web: app shell with a persistent player (mini player while browsing), routing, Login
  (code + QR), Home, Search, Player
- [x] Dockerfile + compose; the image builds and runs on this PC. Running it on the target VM moved to M8.
- Later in M2/M3: lower latency (Twitch prefetch segments), WebSocket events instead of polling `/api/auth`

**Needs from you:** a Twitch app registration (Client ID + secret). ✓ done
**Done when:** on desktop you can log in via phone, see followed live channels, search, and watch
one stream served by the container on the VM.

## M3 — Ad handling v1

- [x] Ad detection module with fixture tests (S1 recordings, sanitized with `scripts/sanitize-playlist.py`)
- [x] Output timeline keyed on `PROGRAM-DATE-TIME` and shared by all renditions:
  backup segments during ads, otherwise slates. Never leaves a gap (S2, D19).
- [x] Backup **chain** (`embed`, `popout`, `autoplay`, `frontpage`), so the real stream plays
  whenever any source is ad-free; the slate only when all are in an ad (D20)
- [x] Slate segments, MPEG-TS and fMP4 (`scripts/make-slate.sh`)
- [x] Client badge: "Ad skipped" (backup) or "Ad break · back in …" (slate), timed to the playback position
- [x] Log which strategy filled each break
- [x] Test switches on `POST /api/play`: `player_type`, `backup`, `simulate_ads` (in the web app via
  `localStorage["nitch.debug.play"]`)

Tested on 2026-11-02 in the browser with simulated ads on a live channel:
- **backup:** seamless, no stall
- **slate** (backups switched off on purpose): the slate plays for exactly the ad stretch, then
  back to live on its own
- **backup chain** (2026-11-03): each source resolved once, `embed` filled all 16 segments of the
  simulated break, no slate

Still open:
- a *real* Twitch ad replaced live (none came up on `site` during the tests)
- the fMP4 slate in the browser (no fMP4 channel was live)
- how often all sources are in an ad at once, read from the per-break logs in real use

**Done when:** an ad break in single view shows either backup content or a clean overlay, and
playback resumes on its own. ✓ (with simulated ads)

## M4 — Multiview

- Sessions stored on the server (SQLite), unlimited: CRUD API, tiles in 60×60 grid cells
- Pipeline manager: one actor per **running** session, shared by every device that opens it;
  add / replace / remove tiles and change layouts at runtime; a slate per tile
- `session_updated` events so all devices watching a session follow edits
- Per-tile audio renditions + master playlist; audio switching on the client
- Encoder/decoder probing (VA-API → software), diagnostics endpoint
- Limit on running sessions, idle teardown (the saved session stays)
- Web: session list (create from template, rename, duplicate, delete), tile focus (audio), swap/remove tiles
- Grid editor:
  - templates as a starting point
  - add streams and place/resize them on the grid with mouse and with the TV remote (snap to cells)
  - outline preview while editing
- Layout-change markers in the playlist, so client overlays switch in sync with the video

**Done when:**
- 2×2 runs stable for an hour on the target
- layout and tile changes don't interrupt the output
- audio switching is instant
- tiles can be placed and resized on desktop and with the TV remote
- a session edited on the desktop opens on the TV with the identical layout

## M5 — Android TV

- 10-foot UI pass: focus styles, spatial navigation on every screen, Back key handling
- Kotlin shell: WebView, Leanback launcher + banner, key mapping, keep screen on, cleartext LAN only
- Server discovery: mDNS + manual entry
- Search with the system on-screen keyboard
- Testing: Android TV emulator + a real device

**Needs from you:** which Android TV device(s) you use.
**Done when:** the whole app is usable with nothing but the TV remote.

## M6 — Chat overlay (read-only)

- Anonymous IRC reader on the server, fanned out over the WebSocket
- Overlay over the video with a slightly transparent background (opacity adjustable), Twitch emotes
  and badges
- Modes: off, focused stream, or combined (one merged window, lines tagged by channel, channels selectable)
- Move, resize and font size (zoom), with the mouse and with the TV remote; saved per device
- All chat changes apply live, from the player's quick menu or from settings, without interrupting playback

**Done when:** while a multiview keeps playing, you can switch between off, focused and combined, and
move, resize and zoom the chat.

## M7 — Polish & operations

- Settings screen: quality, ad strategy order, output resolution, access PIN
- Diagnostics screen
- Install guide and example compose file
- [x] GitHub repository: private, `SlopIMade/Nitch` (D21)
- CI: build, tests, image (GitHub Actions)

## M8 — Deployment on the target VM

Done last, by the user's decision (2026-11-02).

- Target VM hardware check: `vainfo`, CPU, cores/RAM (open from M0)
- VA-API decode/encode in multiview (open from S3)
- Run the image there with compose, log in, an hour-long multiview run

**Done when:** Nitch runs on the VM and the TV uses it there.

## Backlog (after v1)

- Profiles per person
- Sending chat (desktop)
- Third-party emotes (7TV, BTTV, FFZ)
- Go-live notifications (EventSub)
- VODs and clips
- Lower-latency output (LL-HLS, or WebRTC via `webrtcsink`)
- Ad-free playback for Turbo / subscribed accounts
- Native ExoPlayer surface on the TV, if WebView playback is too slow
- Remote access (e.g. via Tailscale)

## Open questions

- **Does VirGL in the target VM expose VA-API decode/encode?** If not, the options are:
  - run the container on the host, or in an LXC container with `/dev/dri`
  - pass the iGPU through with VFIO
  - stay with software encoding, which is probably enough for one 2×2 session (measured in S3)
- Which Android TV device(s)?
- ~~Single view segments: proxy them through the server, or point clients directly at the Twitch CDN?~~
  → directly from the CDN (S2: CORS is open); proxying stays a fallback.

## Risks

| Risk | Impact | Mitigation |
|---|---|---|
| Twitch changes ad insertion or the playback token API | Ad handling or playback breaks | Isolated modules, fixture tests, quick updates |
| Avoiding ads is against Twitch's ToS | Twitch may block techniques | Anonymous playback tokens (the account is not involved); drop + overlay always works |
| No hardware video support in the VM | High CPU use, fewer sessions | Software path treated as first-class; measured early (S3) |
| hlsdemux2 behaviour on stalls and discontinuities | Frozen tiles | `fallbackswitch` + slate, tested in S4; plan B: our own `appsrc` input |
| WebView playback on weak TV hardware | Stutter | Only one stream ever (the server merges); ExoPlayer fallback |
