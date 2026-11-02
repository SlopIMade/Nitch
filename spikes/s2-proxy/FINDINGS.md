# S2 findings — playlist proxy and player behaviour

**Run:** 2026-11-02, ~21:45–21:55, from this PC (WSL), with hls.js 1.x in the built-in browser.
Channels: tarik, lirik, zackrawrr. One evening, one IP: a first observation, not a rule.

## 1. `site` and `embed` share one timeline, segment for segment

`align <channel>` fetches the 480p playlist of both player types at the same time:

| Channel | Same seq numbers | Same `PROGRAM-DATE-TIME` | Newest live segment |
|---|---|---|---|
| tarik | 14/14 | 14/14 | **byte-identical** (368 104 bytes) |
| zackrawrr | 14/14 | 14/14 | **byte-identical** (354 004 bytes) |
| lirik | 0/14 | 2/14 | not comparable: `embed` started a fresh playlist at seq 0 |

- When both playlists are live, they are **the same transcode**; only the URLs differ (signed per
  session). A backup segment can replace an ad segment at the **exact same sequence number**, with
  no visible jump.
- In the lirik case `embed` was most likely in an ad: its playlist restarted at sequence 0 with only
  4 segments. **While a playlist is in an ad, its numbering doesn't match the live one.** Splicing
  must therefore be keyed on `PROGRAM-DATE-TIME` (or on the live playlist's numbering), never on the
  backup's sequence numbers alone.

## 2. CORS is open on the Twitch CDN

Requests with `Origin: http://localhost:8787` get `access-control-allow-origin: *` for **segments
and media playlists**. hls.js played segments loaded straight from the CDN without errors
(`seg=direct`). Routing them through our server (`seg=proxy`) worked just as well.

So in single view, clients can fetch segments directly from Twitch while our server only rewrites
playlists. That costs the server almost nothing.

## 3. Simply dropping ad segments is not good enough

The proxy renumbers its own output (`EXT-X-MEDIA-SEQUENCE` and `EXT-X-DISCONTINUITY-SEQUENCE`), so
dropping segments never confuses hls.js: no fatal errors, and playback resumes on its own at the
`EXT-X-DISCONTINUITY`. But what the viewer sees isn't acceptable. With `sim` mode, which drops 30 s
every 2 min:

| Gap | Buffer before | Freeze | Afterwards |
|---|---|---|---|
| 1st, 30 s | 14 s | **14.6 s** frozen picture | Resumes at the live edge; latency 14.5 s → 4.1 s, only ~3 s buffered |
| 2nd, 30 s | 3 s | **28 s** frozen picture | Two more short stalls (2 s) right after, because the buffer is so thin |

- The freeze is roughly *gap length − buffer*, and the content in between is lost.
- After each gap the player sits right at the live edge with a tiny buffer, so it stutters.
- hls.js only reports a non-fatal `bufferStalledError`, so the client can detect this but not prevent it.

**Consequence for M3:** the output timeline must **never have a hole**. Every dropped ad segment is
replaced by something of the same duration:

1. the **backup** player type's segment for the same `PROGRAM-DATE-TIME` (seamless, per finding 1), or
2. a **slate segment** (pre-encoded "Ad break — back in …" picture, same duration) when no
   backup is live.

That keeps latency and buffer stable, so the player never stalls.

## 4. Smaller observations

- **Latency:** hls.js with `liveSyncDurationCount: 3` started ~14.5 s behind the playlist edge.
  We drop Twitch's `EXT-X-TWITCH-PREFETCH` (low-latency) segments. Exposing them could bring
  latency down, which is a tuning task for M2/M3.
- **One rendition per test:** the spike's master playlist offers a single rendition. With several,
  every rendition needs the **same output numbering**, or hls.js can't switch quality cleanly. In M2
  this means one rewriter per stream that drives all renditions together, keyed on upstream seq/PDT.
- **hls.js `playingDate`** was off by several seconds after a discontinuity. Don't use it to measure
  latency; use the server's own timestamps.

## Not tested yet

- A real ad break replaced by a backup in the player (we only proved segments are identical);
  this happens in M3.
- Slate segments: encoding them and how hls.js handles the codec/resolution change at the discontinuity.
- The TV WebView: same hls.js, expected to behave the same; tested in M5.
