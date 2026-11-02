# S3 findings — 2×2 composite with software encoding

**Run:** 2026-11-02, ~22:00–22:20, in WSL on this PC (Ryzen 9 3900X, 12 cores / 24 threads), GStreamer 1.24.2.

**Pipeline:** 4 × Twitch HLS (`uridecodebin3` → `hlsdemux2`, libav decoders) → scale to 960×540 →
`compositor` → 1920×1080 → `x264enc` (`tune=zerolatency`, 6 Mbit/s, 2 s GOP) → `hlssink2`.
Audio from tile 0 only (AAC 128k).

**Not yet tested:** VA-API on the target VM. That part of S3 is still open (see ROADMAP M0/M1).

## 1. CPU cost

Steady state over 75 s with 4 live channels (tarik, papaplatte, trymacs, zackrawrr), measured as
the process's CPU time per second (1.0 = one fully busy core), with the first 20 s excluded:

| Inputs | x264 preset | Output fps | CPU, avg (max) |
|---|---|---|---|
| 480p | superfast | 30 | **1.56** (1.94) |
| 480p | veryfast | 30 | **1.75** (2.08) |
| 480p | faster | 30 | 2.22 (2.56) |
| 360p | veryfast | 30 | 1.58 (2.10) |
| 720p (mostly 720p60) | veryfast | 30 | 2.83 (3.89) |
| 480p | veryfast | **60** | 3.12 (3.59) |

- **One 2×2 session costs about 1.5–2 cores** of a Zen 2 desktop CPU, with no GPU. Software encoding
  is realistic for one or two sessions; how this translates to the target VM depends on its CPU
  and cores (still open).
- **The output frame rate matters most:** 60 fps nearly doubles the cost. 30 fps stays the default.
- **720p inputs cost ~60 % more** than 480p (decoding 60 fps sources plus bigger frames) for no visible
  gain in a 960×540 tile. Picking the rendition just above the tile size is right.
- 360p saves little over 480p and looks noticeably worse. **480p is the default for 2×2 tiles.**
- The x264 presets differ by only ~0.2–0.5 cores, so decoding and compositing are a large share of the cost.
  VA-API *decode* alone would already help.

## 2. One stalled input holds up everything

Two things went wrong with lirik, whose every fresh session started with a **preroll ad** (playlist
3/3 ad segments, for `site` and `embed` alike):

- In slot 1, lirik's tile **stayed black**. Its first frame arrived after the other three had
  started, and since `compositor` (with `ignore-inactive-pads`) had moved on, all of lirik's frames
  counted as "too late". Shifting a late tile's timeline onto the output position (a pad offset on
  its first frame) is the fix for joining late. It's built in (`--align-tiles`), but a preroll still
  blocked this tile.
- In slot 0 (the audio source), **the whole output stopped** for ~15 s (2 segments), then caught up
  in a burst.

**Root cause:** the pipeline isn't paced by a clock. `hlsdemux2` delivers data in bursts, as segments
are downloaded, and the compositor and HLS sink move as fast as their inputs allow. A slow input slows
everything down, and an input that has a backlog runs ahead. The same thing explains the
"1.2–1.3× realtime" in every run: at startup, each input first plays back the ~28 s already in its
playlist as fast as possible.

**Consequence for S4/M4:** each tile must be a **live, clock-paced source with its own fallback**,
and the output must be driven by the pipeline clock, never by one input:

- re-timestamp each tile onto the pipeline clock when it starts or restarts (`fallbacksrc` /
  `livesync` from gst-plugins-rs, or our own offset logic as here)
- on a stall, show a slate for that tile (`fallbackswitch`) while the others keep running
- give the output audio its own timeline (silence fallback), so it never depends on one tile
- start each input near the live edge instead of replaying the playlist backlog

## 3. Output works

- `hlssink2` writes valid 2 s MPEG-TS segments with H.264 + AAC.
- Startup to the first segment: ~1 s.
- The 2×2 picture checks out: a frame decoded from a segment showed all four streams, correctly
  placed and scaled.
