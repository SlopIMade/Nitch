# S4 findings — clock-paced multiview with runtime changes

**Run:** 2026-11-02, ~22:40–23:10, in WSL on this PC (Ryzen 9 3900X), GStreamer 1.24.2 +
gst-plugins-rs 0.15 (`fallbacksrc`), x264 veryfast, 1080p30 output on the 60×60 grid (D17).

## Pipeline that works

```
per tile:  fallbacksrc (uri = Twitch 480p, fallback-uri = slate.png, timeout 3 s)
             ├─ video ─► compositor.sink_N   (xpos/ypos/width/height from grid cells, keep-aspect-ratio)
             └─ audio ─► audioconvert ─► audioresample ─► S16LE/48k/2ch ─► audiomixer.sink_N (volume 0/1)

pacing:    videotestsrc is-live (16×16 black, zorder 0) ─► compositor     ← the clock sets the pace
           audiotestsrc is-live (silence)               ─► audiomixer

output:    compositor ─► I420 1920×1080@30 ─► videoconvert ─► queue ─► x264enc ─► video/x-h264,profile=high
           audiomixer ─► AAC                    ─► hlssink2
```

## 1. The output never stops anymore

The scripted 100 s run, with every change made while the output was running:

| t | Change | Result |
|---|---|---|
| 0 s | 2×2 with 4 channels | all 4 `running` after ~4 s |
| 25 s | Layout → 1 big (40×40) + 3 small (20×20) | correct ([frame](#frames)) |
| 40 s | Replace tile 1 with another channel | new channel in place, others unaffected |
| 55 s | Replace tile 3 with a **dead URL** | `fallbacksrc` → `retrying`, tile shows the **slate** |
| 70 s | Back to 2×2 and put a channel on tile 3 | joins late, correctly in place and in time |
| 90 s | Audio focus tile 0 → tile 2 | output audio changes source (−21 dB → −50 dB, a quieter streamer) |

- **0.98× realtime**, 51 segments = 102 s of video in 104 s. No bursts: the clock drives the
  output, not the inputs. This fixes the main problem from S3.
- **Brief hiccup after a change:** the next segment arrives ~1 s late (3.0 s instead of 2 s).
  The video timeline stays continuous, so a normal player buffer (≥ 6 s) absorbs it. Worth tuning in M4.
- **A late tile joins correctly** without any offset logic of our own: `fallbacksrc` is live and
  timestamps against the pipeline clock.

## 2. CPU — and a trap

| Variant | CPU (cores, avg) |
|---|---|
| First attempt (1080p black background, formats left to negotiation) | 6.30 |
| Tiny 16×16 live background | 4.85 |
| **+ pinned formats (I420 into the compositor, H.264 High out of the encoder)** | **1.85** (scenario: 1.98) |

- **The trap:** left to negotiation, the compositor picked a 4:4:4 format (because of `fallbacksrc`'s
  conversions), and x264 encoded **High 4:4:4 at 10 bit**. That is slow, and **browsers and TVs can't play
  it**. Rule for M4: always pin the raw format (I420) before the encoder and the H.264 profile after it,
  and test the output profile.
- **The live background** only has to be live, not big. A 1080p background costs ~1.4 cores of blending.
- With both fixes, `fallbacksrc` + clock pacing costs about the same as the unprotected S3 pipeline
  (1.75 cores), so the robustness comes almost free.
- The per-thread breakdown (threads are named after elements) was the fastest way to find this. It
  should be part of the diagnostics endpoint.

## 3. Details for M4

- **Replacing a tile:** set the old `fallbacksrc` + audio chain to `NULL`, remove them, release
  their compositor/mixer pads, add the new ones, then `sync_state_with_parent`. No glitch in other tiles.
- **Layout:** set the pad properties `xpos/ypos/width/height` from grid cells × 32/18 px, with
  `sizing-policy=keep-aspect-ratio` for "fit". This applies on the next frame.
- **`fallbacksrc` `status`** (`buffering` / `running` / `retrying` / `stopped`) is what
  `tile_state` events to the client should carry.
- The audio mixer with volume 0/1 was only a stand-in; M4 uses one audio rendition per tile, as
  planned (D7).

## Frames

Grabbed from the output segments during the run (not committed): 2×2, 1 big + 3 small,
tile 1 replaced, tile 3 showing the "Stream unavailable" slate, 2×2 with the late channel.

## Not tested yet

- VA-API decode/encode on the target VM (open from S3)
- A tile *stalling mid-stream*, as opposed to failing at start, which mostly happens with ad breaks;
  in production the inputs are our ad-handled playlists (M3) anyway
- Long-term stability (1 h+), which is an M4 "done when" criterion
