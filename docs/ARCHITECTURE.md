# Architecture

## Goals (v1)

- Watch Twitch in a desktop browser and on Android TV (fully usable with a TV remote).
- **Multiview:** several live streams merged by the server into one video.
  - Tiles are placed on a fixed **60×60 grid**.
  - Multiview **sessions are saved on the server**, as many as you like.
  - Every device sees the same sessions with the identical layout.
- **Ad breaks** are detected and handled by the server.
- **One shared Twitch login** held by the server; on the TV you log in with your phone.
- Home screen with followed live channels, channel search.
- **Chat overlay** (read-only):
  - modes: off, focused stream, or one combined chat from several streams
  - floats over the video
  - adjustable size, position and font size
  - can be changed while the streams keep playing
- Runs as a Docker container on Linux, inside the home network.

Not in v1 (see [Roadmap → Backlog](ROADMAP.md#backlog-after-v1)): profiles per person, sending chat,
VODs/clips, notifications, remote access.

## Overview

```
                 Twitch  (Helix API · GQL · usher HLS · CDN · chat)
                                  ▲
                                  │ HTTPS
 ┌────────────────────────────────┴───────────────────────────────┐
 │ Nitch server — Docker on Linux                                 │
 │                                                                │
 │  nitch-twitch   login (device code), Helix, playback tokens,   │
 │                 chat reader                                    │
 │  nitch-hls      playlist proxy, ad detection, ad strategies    │
 │  nitch-media    GStreamer: decode → compositor → encode → HLS  │
 │  nitch-server   REST + WebSocket API, serves the SPA, mDNS     │
 └───────────────┬────────────────────────────────┬───────────────┘
                 │ HTTP (LAN)                     │ HTTP (LAN)
        ┌────────┴─────────┐             ┌────────┴──────────┐
        │ Desktop browser  │             │ Android TV APK    │
        │ React SPA        │             │ WebView → same SPA│
        └──────────────────┘             └───────────────────┘
```

The client always plays **exactly one** HLS stream: either one Twitch stream passed through, or
the server's merged multiview. That keeps the client light enough for cheap TV hardware.

## Components

### Server (`server/`, Rust)

Cargo workspace, async runtime **tokio**, HTTP via **axum**, media via **gstreamer-rs**.

| Crate | Responsibility |
|---|---|
| `nitch-server` | Binary. Config (env), HTTP/WebSocket API, serves the built SPA, mDNS advertisement, SQLite store, wiring. |
| `nitch-twitch` | OAuth device code login + token refresh, Helix client, GQL playback access token + usher URLs, chat reader. |
| `nitch-hls` | HLS proxy: polls Twitch playlists, rewrites them, detects ads, applies ad strategies, emits events. |
| `nitch-media` | GStreamer multiview pipelines: tile inputs, layouts, compositor, encoders, HLS output, capability probing. |

Each multiview session is an **actor**: one tokio task owns the GStreamer pipeline and receives
commands (`SetLayout`, `ReplaceTile`, `RemoveTile`, `Stop`) over a channel. The API never touches a
pipeline directly.

Elements from **gst-plugins-rs** (`fallbackswitch`, `hlscmafsink`, later `webrtcsink`) are pulled in
as Rust crates and registered statically, so the Docker image only needs stock GStreamer packages.

### Web client (`web/`)

**React + TypeScript + Vite**, with:

- **Norigin Spatial Navigation**: focus handling for the TV remote (D-pad, OK, Back)
- **hls.js**: playback, including audio track switching
- **TanStack Query**: API data and caching
- a QR code library for the login screen

There is one UI for desktop and TV. Inside the TV shell it switches to a "10-foot" mode: larger type,
visible focus rings, no hover-only controls. Everything you can click is also focusable.

The **player is mounted at the app root** and stays mounted when you navigate. Menus, search and
settings open as layers on top of it, so changing a setting never stops the streams.

| Screen | Content |
|---|---|
| Connect | TV only, when no server is known: discovered servers + manual address. |
| Login | Device code + QR code ("open twitch.tv/activate on your phone"). |
| Home | Followed channels that are live: thumbnail, title, category, viewers. |
| Search | Channel search, live channels first. TV uses the system on-screen keyboard. |
| Player | Single stream, quality, ad-break overlay, chat overlay. |
| Sessions | Your saved multiview sessions (any number, same list on every device): open, create (from a template), rename, duplicate, delete. |
| Multiview | Plays a session. The editor adds streams from follows/search and places them on the 60×60 grid (move, resize, snap); the focused tile gets audio. |
| Settings | Opens over the running player and changes apply live. Covers chat (mode, channels, opacity, font size), default quality, ad strategy order, output resolution, encoder info and logout. |

### Android TV shell (`android-tv/`)

Kotlin, a single Activity with a WebView. The UI is not bundled in the APK; it is loaded from the server.

- Leanback launcher entry + banner, so it appears on the TV home screen.
- Finds the server via mDNS (`_nitch._tcp`) or manual entry, and remembers it.
- Loads `http://<server>:<port>/` (cleartext allowed for local addresses only).
- Remote keys: D-pad/OK arrive as arrow keys/Enter (WebView default). **Back** is forwarded to the
  web app, which decides between "go back" and "exit". Media keys are forwarded as JS events.
- Small JS bridge: keep the screen on during playback, app version, exit app.
- Plan B if WebView playback turns out too slow: a native ExoPlayer surface behind the web UI.

## Playback paths

### Single view: passthrough, no re-encoding

1. Client: `POST /api/play {channel}` → server answers with `/hls/<id>/master.m3u8`.
2. Server requests a playback access token (GQL, anonymous) and the usher master playlist
   (H.264 renditions; HEVC/AV1 renditions ignored for compatibility).
3. Server rewrites the master and media playlists so they point to itself, with its **own sequence
   numbering**, shared by all renditions so quality switches stay aligned. Ad segments are handled
   by `nitch-hls`. Live segments point **directly to the Twitch CDN**: CORS is open (S2), so the
   server never touches the video bytes. Proxying stays available as a fallback setting.
4. hls.js plays it with normal adaptive bitrate switching.

This costs almost no CPU on the server.

### Multiview: composite

Each tile uses the **same ad-handled proxy playlist** as single view, consumed internally. For each
tile the server picks the rendition just above the tile size, e.g. 480p or 720p for a 2×2 tile. That
saves both decoding and bandwidth.

One GStreamer pipeline per running multiview session. This is the structure proven in spike S4:

```
per tile N:
  fallbacksrc (uri = internal ad-handled playlist, fallback-uri = slate) ─┬─► video ─► compositor.sink_N
                                                                         │       (grid rect, keep-aspect-ratio)
                                                                         └─► audio ─► convert/resample ─► AAC ─► audio rendition N

pacing (live, so the clock - never a tile - drives the output):
  videotestsrc is-live, 16×16 black, zorder 0 ─► compositor
  audiotestsrc is-live, silence               ─► every audio rendition (fallback)

output:
  compositor ─► I420 1920×1080@30 ─► H.264 encoder (VA-API or x264) ─► profile=high ─► hlscmafsink (video)
  audio rendition 1..N                                                             ─► hlscmafsink (one per tile)
  server writes master.m3u8: 1 video + N audio renditions (EXT-X-MEDIA TYPE=AUDIO)
```

- **Layout change:** set the compositor pad properties (`xpos`, `ypos`, `width`, `height`, `zorder`)
  while the pipeline runs. No restart.
- **Add, replace or remove a tile:**
  1. set the old `fallbacksrc` and its audio chain to `NULL` and remove them
  2. release their pads
  3. add the new ones and call `sync_state_with_parent`

  The other tiles don't notice (S4).
- **Stalls and failures** (streamer offline, network trouble, source can't start): `fallbacksrc`
  shows that tile's slate and keeps retrying. The other tiles are not affected. Its `status`
  (`buffering` / `running` / `retrying`) feeds the `tile_state` events.
- **Pinned formats:** the raw format before the encoder (I420, 8 bit) and the H.264 profile after it are
  always set explicitly. Left to negotiation, S4 ended up with High 4:4:4 10-bit: 3× the CPU and
  unplayable in browsers. The output profile is checked in tests.
- **Clock-paced, not data-paced:** S3 showed that without this, one stalled input (e.g. a preroll)
  holds up the whole output. Therefore:
  - every tile is re-timestamped onto the pipeline clock when it starts or restarts
  - the output (compositor, encoder, audio) runs on the clock and never waits for any single tile
  - inputs start near the live edge instead of replaying the playlist backlog
- **Audio:** every tile has its own audio rendition. The client switches audio tracks in hls.js
  instantly, with no server round trip and no pipeline delay.
- **The grid:** the output is a fixed 16:9 canvas divided into **60 × 60 cells**. At 1080p a cell is
  exactly 32 × 18 px.
  - Every tile is a rectangle in whole cells `{x, y, w, h}` plus z-order. Overlaps are allowed; z
    decides what's in front.
  - **Equal width and height in cells means 16:9 in pixels.** A 30×30 tile is a quarter, a 40×40 tile
    is two thirds, both the exact shape of a stream.
  - 60 divides into halves, thirds, quarters, fifths and sixths, so common layouts line up without remainders.
  - A stream whose tile isn't 16:9 is fitted inside it (letterboxed). "Fill" (crop) is a per-tile option.
  - Because the server draws the grid into the video, the layout is **identical on every device**.
    A screen with another aspect ratio shows the same 16:9 picture with black bars.
- **Sessions:** a multiview session is a saved, named arrangement of tiles (channel + grid
  rectangle), stored on the server. There is no limit on the number of sessions.
  - Templates (1+1, 2×2, 1 big + 2 small, 1 big + 3 small) are only starting points for new sessions.
  - **Opening a session** starts its pipeline, or joins it if it's already running: two devices on the
    same session share one pipeline and see the same picture. Audio track and chat stay per device.
  - **Editing a session** on any device updates the saved session and the running pipeline. Other devices
    watching it receive a `session_updated` event, so their overlays follow.
- **Editing feedback:** the video reaches the client a few seconds after the server composites it.
  While you drag or resize, the editor draws outlines over the video immediately. The server applies
  the new layout when you let go, and the video catches up a few seconds later.
- **Overlays in sync:** the client draws focus frames and tile labels from the same layout data.
  A marker in the playlist (`EXT-X-DATERANGE`) tells it where in the video a new layout begins, so
  the overlays switch together with the picture.
- **Defaults:** output 1920×1080 at 30 fps, H.264, configurable. Tiles take the rendition just above
  their size (480p for 2×2). Measured in S3: about 1.75 cores of a Ryzen 9 3900X with x264 veryfast; 60 fps output
  nearly doubles that, and 720p inputs add ~60 %.
- **Limits:** saved sessions are unlimited, but the number of sessions **running** at once is
  configurable, since each one costs ~2 cores in software. A running pipeline is torn down when no
  client has fetched it for N seconds; the saved session stays.

**Encoders and decoders** are probed at startup, in order: VA-API (`vah264enc` / `vah264lpenc`, `vah264dec`),
then software (`x264enc` with a fast preset and zerolatency tuning, libav decoders). The chosen chain
is shown in diagnostics.

**Latency:** Twitch's own latency, plus roughly 3–6 s for multiview (segment length × buffered
segments). LL-HLS or WebRTC (`webrtcsink`) can bring this down later.

## Chat overlay (client)

The client draws the chat as HTML on top of the video; it is not baked into the video. That keeps it
sharp at any zoom, lets it change instantly, and costs the server nothing.

- **Modes:** off, focused stream, or combined.
  - Combined is **one merged window**: messages from the selected streams in arrival order, each line
    tagged with its channel (color stripe + name).
  - In multiview you pick which streams are included; the default is all tiles.
  - In single view, focused and combined are the same.
- **Look:** floats over the video with a slightly transparent background; the opacity is adjustable.
- **Geometry:** position and size are a rectangle relative to the video area, so it scales with the
  screen. Font size (zoom) is set separately from the window size.
- **Editing:**
  - Desktop: drag to move, drag the edges to resize, Ctrl + mouse wheel for font size.
  - TV: an "Adjust chat" mode from the player menu. The D-pad moves the chat, OK switches between
    move and resize, a small toolbar sets the font size, and Back finishes.
- **Live changes:** mode, channels, geometry and font size change without touching the stream, either
  from the player's quick menu or from settings (which open over the player).
- **Saved per device** in localStorage, because a TV and a desktop monitor need different sizes.
- **Light on TV hardware:** only the last ~150 messages stay in the DOM, and message bursts are batched
  into one update per animation frame.

## Ad handling (`nitch-hls`)

Twitch inserts ads into the HLS stream on its own servers. During an ad break the ad **replaces** the
live content for that viewer, so there is no original video underneath to recover. "Removing ads"
therefore means **detect, then substitute**.

**Detection.** These are the markers observed in spike S1
([findings](../spikes/s1-twitch-playback/FINDINGS.md)), in order of reliability. Expect them to change:

1. `#EXT-X-DATERANGE` with `CLASS="twitch-stitched-ad"`. It appears about 4 s *before* the first ad
   segment and carries `DURATION` and `X-TV-TWITCH-AD-POD-FILLED-DURATION` (the length of the whole
   break). It is mapped onto segments via `START-DATE` and the segments' `PROGRAM-DATE-TIME`.
2. Segments whose `#EXTINF` title is not `live` (ads are titled `Amazon|<creative id>`).
3. The `twitch-stream-source` DATERANGE switching from `live` to something else.

`twitch-maf-ad` DATERANGEs are display ads that the web player shows next to the video. They don't
affect the segments and are ignored.

All detection rules live in one module, covered by tests that use recorded playlists (fixtures).
When Twitch changes something, only that module and its fixtures need updating.

**Strategy chain** (per stream, order configurable). The primary playlist uses player type `site`,
which got the fewest video ads in S1.

1. **Backup playlist:** request a second playback token with a different player type. In S1, `site`
   had live content while `embed` and `picture-by-picture` were in ad breaks; those two behaved
   identically, so they count as one backup source. While the main playlist is in an ad break, its
   ad segments are replaced by the backup's segments with the same `PROGRAM-DATE-TIME`. In S2 those
   were **byte-identical** to the main playlist's live segments, so the swap is seamless. Keyed on
   PDT, not on sequence numbers: a playlist in an ad break numbers its segments differently.
2. **Slate + notify:** if no backup is live, replace each ad segment with a pre-encoded **slate
   segment** of the same duration ("Ad break — back in …"), and send `ad_break {channel, duration}`
   over the WebSocket so the client can show a countdown. In multiview, the tile shows its slate,
   and audio can automatically move to another tile (setting).

**Never leave a hole.** S2 showed that simply dropping ad segments freezes the picture for
*(gap − buffer)* seconds, after which the player sits at the live edge with a tiny buffer and stutters.
The output timeline therefore always stays continuous.

Playback tokens are always requested **anonymously**, never with the user's account.

## Twitch integration (`nitch-twitch`)

- **App registration:** you register an app at dev.twitch.tv; its Client ID and secret go into the
  server environment. It is registered as a *confidential* client. Refresh tokens of public clients
  expire after 30 days (verify in M2).
- **Login:** OAuth device code grant flow.
  1. The server starts the flow.
  2. The client shows the `user_code` and a QR code of the verification URL.
  3. The server polls until you confirm on your phone, then stores the tokens in SQLite and refreshes them.

  There is one shared account: logging in on any device logs in all devices. Clients never see the tokens.
- **Scopes:** `user:read:follows`, plus `user:read:chat` if chat later moves to EventSub.
- **Helix (official API):** Get Followed Streams, Search Channels, Get Users, Get Streams.
- **GQL (unofficial, the same thing the browser player and streamlink use):** the playback access token
  and the usher master playlist. This is isolated in its own module because it can break without notice.
- **Chat (read-only):** anonymous IRC over WebSocket (`irc-ws.chat.twitch.tv`). One connection that
  joins and leaves channels as the shown chats change (focused stream or combined set) and parses tags (emotes, badges, colors); messages are fanned
  out over our WebSocket. It sits behind a trait so EventSub can replace it.

## Server API (sketch)

REST + JSON under `/api`:

| Method | Path | Purpose |
|---|---|---|
| GET | `/api/health` | Health and version |
| GET | `/api/auth` | Login state and user |
| POST | `/api/auth/device` | Start device login → `{user_code, verification_uri, expires_in}` |
| DELETE | `/api/auth` | Log out |
| GET | `/api/follows/live` | Followed channels that are live |
| GET | `/api/search?q=` | Channel search |
| GET | `/api/channels/{login}` | Channel and live info |
| POST | `/api/play` | Single view → `{hls_url}` |
| GET | `/api/sessions` | All saved multiview sessions |
| POST | `/api/sessions` | Create `{name, tiles[]}` (tile: `{channel, x, y, w, h, z, fit}` in grid cells) |
| GET | `/api/sessions/{id}` | One session |
| PATCH | `/api/sessions/{id}` | Rename, or change tiles; a running pipeline applies it live |
| DELETE | `/api/sessions/{id}` | Delete a session (stops its pipeline) |
| POST | `/api/sessions/{id}/play` | Start or join its pipeline → `{hls_url}` |
| GET | `/api/system` | Diagnostics: encoder chain, sessions, load |

WebSocket at `/api/ws`:

- **Server → client:** `auth_changed`, `ad_break`, `ad_break_end`, `tile_state` (loading / live / offline), `session_updated`, `chat_message`.
- **Client → server:** `chat_subscribe {channels[]}`. Every `chat_message` carries its channel, which
  is what the combined chat needs.

Playlists and segments are served under `/hls/...`.

## Storage

A SQLite file in the `/data` volume holds tokens, server settings and the multiview sessions
(name + tiles in grid cells). The Twitch client secret comes from the environment, not from the
database.

Per-device UI settings (chat mode, chat geometry, font size, opacity) live in the client's
localStorage.

## Deployment

- **One Docker image**, built in stages:
  1. Build the Rust server against the GStreamer dev libraries.
  2. Build the web app.
  3. Runtime image: same distro release as the build stage, with the GStreamer runtime, VA drivers,
     our binary, and the SPA in `/app/web`.
- **compose:**
  - `network_mode: host` (needed for mDNS)
  - volume `/data`
  - env file
  - optional `devices: [/dev/dri]`
- **Target:** a Linux VM with an AMD iGPU passed through via QEMU VirGL. Hardware video support there
  is **not verified yet** (see [Roadmap → M0](ROADMAP.md#m0--foundations)). Software encoding is
  always available as a fallback.

## Security

- LAN only; never port-forward the server.
- The server holds the Twitch tokens and never hands them to clients.
- Any device on the LAN can use the API. v1 offers an optional access PIN.
