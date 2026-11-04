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
*Superseded for the image by D32 (2026-11-04): it is Debian 13 with GStreamer 1.26; dev stays on
Ubuntu 24.04.*

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

### D18 — Tiles are `fallbacksrc`; live sources pace the output; formats are pinned
*2026-11-02*

From S3/S4:
- Each tile is a `fallbacksrc` (live, clock-timestamped, slate on failure, automatic retry).
- A tiny live background (video) and live silence (audio) pace the compositor and mixers, so no
  tile can hold up the output.
- I420 before the encoder and H.264 High after it are set explicitly.

Measured: all runtime changes work at 0.98× realtime, for ~1.85 cores per 2×2 session in software.
**Rejected:**
- Data-paced pipeline (S3): one preroll froze the whole output.
- Our own re-timestamping: `fallbacksrc` already does it, plus fallback and retry.
- A full-size background: ~1.4 cores of blending for nothing.

### D19 — Single-view output is a time-keyed timeline, not a 1:1 segment swap
*2026-11-02*

Recorded ad breaks have 32 ad segments where live content has 30 (each ad ends with a 0.235 s
remainder), and some channels use 1.985 s segments, so swapping segment for segment drifts. Each
play session therefore builds one timeline shared by all renditions:
- entries are keyed by `PROGRAM-DATE-TIME` and get our own numbering
- each stretch takes the primary's live segment, else the backup's, else a slate (TS or fMP4)
- discontinuities only around slates

Refines D15.
**Rejected:**
- 1:1 swap by sequence number or PDT: drifts after the first ad.
- Keeping Twitch's numbering: shifts during ads.

### D20 — The real stream from a backup chain; the slate only as the last resort
*2026-11-03*

The user wants the actual stream during ads, not a placeholder. During an ad the server therefore
fetches the same rendition from several backup player types in priority order (`embed`, `popout`,
`autoplay`, `frontpage`), each its own Twitch session with its own ad schedule. The first one with
the live segment wins. It only costs a playlist request per source and refresh, never video.

The slate appears only if every source is in an ad at the same moment. The user chose the slate
over showing Twitch's ad in that case. Per-break logs show how often it still happens.
**Rejected:** a single backup, because in S1 `embed` and `picture-by-picture` had their ads at the
same moments.

### D21 — Private GitHub repository
*2026-11-03*

The repository is pushed to `SlopIMade/Nitch`, a **private** repo the user created; `main` is
the default branch. Before the first push, the whole history was scanned: no credentials, tokens,
signed Twitch URLs or session IDs. Raw recordings (`data/`) and `.env` stay git-ignored.
Replaces "local git only" from D9. CI comes in M7.

### D22 — Multiview tiles run without slates; the client draws tile states
*2026-11-03*

Tile inputs are internal play sessions (so tiles get the backup chain, D20), but **without slates**:
- In the browser test, a tile that started in a preroll stayed black after the break. The slate
  segments' restarting timestamps confused its timeline in the compositor.
- Without slates, the tile's playlist simply pauses while no source is ad-free, and `fallbacksrc`
  bridges the gap.

The state of each tile (waiting, ad break, reconnecting, offline) is drawn by the client over the
video, from `GET /api/sessions/{id}/play`. A fallback image inside `fallbacksrc` failed to negotiate
and kept retrying in the background.
Refines D15 (which still holds for single view in the browser).

### D23 — A moved or resized tile keeps its source
*2026-11-03*

Each new tile gets the rendition just above its pixel size. A tile that is only moved or resized
keeps its source and rendition: switching the rendition restarts the tile's source, which left it
black for ~15 s in the test. A seamless upgrade (start the better rendition hidden, swap when it
runs) is noted for later.

### D24 — Geometric spatial navigation instead of Norigin
*2026-11-03*

For the TV remote, the arrow keys move the browser focus to the nearest focusable element in that
direction (`web/src/tv.ts`). Every existing button, link and input works without wrapping
components, which Norigin (D3) would have required on every screen. Components that use the arrows
themselves (the multiview tile focus, the grid editor, chat adjustment) claim them, optionally
only for some keys or while the focus is elsewhere.
**Rejected:** Norigin Spatial Navigation, which means a rewrite of every screen for a UI this simple.

### D25 — Multiview pipelines run in worker processes
*2026-11-03*

A segfault in native code (GStreamer/plugins) when tiles' streams ended took down the whole server.
It was reproduced in about 1 of 20 test runs, and can't be caught in Rust. Each pipeline therefore
runs in its own process (`nitch-server multiview-worker`), controlled with JSON lines over stdin/stdout.
A crash ends only that pipeline, and it is restarted while someone watches.
Tested by sending `SIGSEGV` to a worker: the server kept answering, and a new worker took over in ~10 s.

### D26 — Multiview pipelines keep real time on a calibrated clock
*2026-11-03*

In a live multiview, playback stuttered (~50 ms every few seconds) and the delay grew to minutes.
Measured: the output carried only ~0.93 s of media per real second. Under WSL2 the monotonic
clock, which GStreamer uses by default, ran ~8 % slow; WSL corrects the wall clock by stepping
it ~2.9 s forward every ~35 s. Players on other machines play in real time, so they ran dry again
and again.

First fix: the wall clock (`clock-type=realtime`) as the pipeline clock. The rate was right, but
every step left 1.5–2.5 s of silence in all audio tracks at once: the mixers raced ahead while
the tiles' audio arrived "late" and was dropped.

The decision (`nitch-media/src/clock.rs`): the pipeline clock is the monotonic clock, calibrated
to the wall clock's rate. The rate is the least-squares slope of wall time over monotonic time
over the last 5 minutes, which averages the steps out. The server measures it from its start
(`/api/system` shows `clock_rate`, 1.09 on the dev PC) and hands it to every new pipeline, which
keeps refining it. On a normal Linux host the rate is 1.0 and nothing changes. hls.js also catches
up on its own (`maxLiveSyncPlaybackRate` 1.08, jump beyond 8 segments), so a stall can never add
lasting delay.

Measured after the change: 1.00 s of media per second, no gaps in any audio track.

### D27 — The player is full screen only, with a side menu instead of pages
*2026-11-03*

From the first test on the TV: the mini player got in the way, Home and Multiview were two
places for one thing, and the remote could only switch tiles. Now:
- The player always covers the screen; Back or ✕ ends playback. Home shows the saved multiview
  sessions and the live follows.
- While watching, a side menu (hold ← on the remote, or "＋ Streams") adds streams live: search,
  recommended (`@name` mentions in the titles of the streams in view), followed live channels.
  OK watches a stream alone, holding OK (or the + button) adds it. A single view turns into a new
  multiview session. Layouts adapt on their own (`web/src/layout.ts`).
- The old stream keeps playing until the new one is ready, so changes are never silent.
- Chat settings live in the same menu (usable with the remote); the overlay itself has no
  buttons, only a drag edge and corner on hover. Messages are kept per channel, so mode changes
  lose nothing.
- Back is a stack of handlers (`installBackHandler`): what opened last closes first. Android 13+
  delivers Back to the TV app only through `OnBackInvokedCallback`, which forwards it to the page.
- Long presses are timed (450 ms from the first keydown): Android's WebView repeats a held key
  only sporadically and never sets `repeat`.

This replaces the mini player part of D14; the player still never unmounts on navigation.

### D28 — Delay: 1 s output segments, tiles start 4 s behind live
*2026-11-03*

The multiview showed about 20 s of delay: Twitch's own (a few seconds), each tile's demuxer
starting three segments (~6 s) behind the live edge, the compositor (0.5 s), 2 s output segments,
and the player staying three segments (6 s) behind the end of our output.

- Output segments are 1 s (a keyframe every second), so the player stays ~3 s behind.
- The tiles' playlists carry `#EXT-X-SERVER-CONTROL:HOLD-BACK=4.0`, which `hlsdemux2` honors: it
  starts two Twitch segments behind the edge.

Measured: the player sits 3 s behind the output's edge (was ~6–8 s). Expected total: ~12–14 s.
Further steps if needed: Twitch's prefetch segments for the tiles, a shorter player distance.

### D29 — Layouts are edited in the player; tiles are areas
*2026-11-03*

Editing a running multiview used to leave the player. Now "Layout" (bottom bar or side menu) turns
the tiles into editable outlines over the running picture; every change is saved and applied live,
and the audio keeps playing. With the remote: pick a tile (‹ ›), "Move" or "Resize" hands it the
arrows until OK. The separate editor page and the layout templates are gone (2026-11-03): sessions
are created by adding streams while watching.

A tile is now an area of any shape; its 16:9 picture is centered inside (the compositor's
`keep-aspect-ratio`). Free shapes are the default, Shift keeps 16:9, "16:9" shrinks an area to
its picture. A tile's source rendition follows the picture's height, not the area's.

### D30 — Tiles use the cheapest fitting rendition, switched without a gap
*2026-11-03*

A tile reads the lowest rendition reaching 80 % of its picture's height (a slight upscale isn't
visible), and 30 fps before 60 fps of the same height, since the output runs at 30 fps. A quarter
tile (540 px) reads 480p30 instead of 720p60: far less to download and decode.

When a resize calls for another rendition, the new source starts muted, right underneath the
current one in the same place (stacking, not transparency: a transparent pad's frames weren't
composited, and the tile stayed black for seconds after the swap). It takes over once it has
delivered frames for 1.5 s; "frames arrived" is a probe on its compositor pad, since fallbacksrc's
status says "buffering" near the live edge even while frames flow. Hidden sources get no
placeholder frames (no immediate fallback, a 15 s timeout), so a placeholder can't pass for the
stream; one that shows nothing within 12 s is dropped and the old source stays. The layout itself
changes at once. Measured: no black frames in either direction. This replaces D23.

### D31 — Chat history from reading the followed channels in the background
*2026-11-03*

Twitch's API has no chat history. Third-party services (recent-messages) would learn which
channels are watched, so the server does it itself: it reads the chats of the followed live
channels (up to 50, refreshed every minute; anonymous IRC, cheap) and keeps the last 150 messages
of every chat it reads. A client subscribing to a channel first gets `chat_history`, so the chat
opens with what was said. Limit: a channel opened from search, without following it, starts empty.

### D32 — The image is Debian 13, like the target VM; data on the NAS
*2026-11-04*

The target VM runs Debian 13 with a virtio GPU in AMD native-context mode. VA-API there decodes
and encodes H.264, but only with a Mesa built with the AMD virtio driver (the VM's own
`+virtio` packages; Debian's Mesa lacks it). So the image is Debian 13 as well, with Mesa
from trixie-backports, and driver packages placed in `deploy/debs/` (git-ignored) replace
Debian's at build time. The image's GStreamer is 1.26, dev stays on 1.24 (Ubuntu 24.04, D11).

Data follows the server's convention: an NFS volume on the NAS
(`/export/nitch`), the multiview output in a tmpfs, since it is rewritten
every second and only needed for a minute. SQLite keeps WAL, but in exclusive locking mode, so it
needs no shared-memory file (which doesn't work on NFS); a second server on the same data fails
at once instead of corrupting it. The stack is `deploy/stack.yaml`; the image is built on the VM.

