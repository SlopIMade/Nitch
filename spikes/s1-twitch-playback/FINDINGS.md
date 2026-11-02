# S1 findings — Twitch playback and ad markers

**Run:** 2026-11-02, 20:59–21:43 (40 min), from one home IP in Germany, anonymous.
5 watchers in parallel:

| Channel | Player type | Ad breaks (in the video) | Ad segments |
|---|---|---|---|
| tarik | `site` | 0 | 0 |
| tarik | `embed` | 3 | 96 |
| tarik | `picture-by-picture` | 3 | 96 |
| lirik | `site` | 1 (a single 6 s ad) | 4 |
| zackrawrr | `site` | 0 | 0 |

**One evening, one IP:** treat the numbers as a first observation, not as a rule. Twitch varies
this by region, time, channel and experiments.

## 1. Playback without login works

- The GQL `PlaybackAccessToken_Template` query (web player Client-ID, no user token) returns a token,
  and usher returns the master playlist. No integrity token was needed.
- The token reports `server_ads=true show_ads=true` for every player type, so its flags don't tell
  us who actually gets video ads.
- Renditions: 1080p60, 720p60, 480p, 360p, 160p and audio_only. All are H.264 Main
  (`avc1.4D401F`) with AAC audio.
- Media playlists:
  - 2 s segments; the window holds about 14 segments (~28 s)
  - every segment has `EXT-X-PROGRAM-DATE-TIME`
  - 1–2 low-latency `EXT-X-TWITCH-PREFETCH` segments at the end
- An offline channel returns 404 from usher.

## 2. How an ad break looks

At the start of a break, in order:

1. **About 4 s early:** an `EXT-X-DATERANGE` with `CLASS="twitch-stitched-ad"` appears while the
   segments are still `live`. Its `START-DATE` lies just ahead, in the prefetch area. **We know an ad is
   coming before its first segment shows up.**
2. An `EXT-X-DISCONTINUITY` and then segments titled `Amazon|<creative id>` instead of `live`.
3. The `twitch-stream-source` DATERANGE switches from `live` to `Amazon|<creative id>`, and back
   to `live` afterwards.

Useful attributes on the ad DATERANGE (`X-TV-TWITCH-AD-*`):

| Attribute | Example | Use for us |
|---|---|---|
| `DURATION` | `30.235` | Length of this ad |
| `POD-LENGTH` / `POD-POSITION` | `2` / `0`, `1` | Number of ads in the break and which one this is |
| `POD-FILLED-DURATION` | `60` | Length of the whole break: the countdown for the overlay |
| `ROLL-TYPE` | `MIDROLL` | Preroll vs. midroll |

There are many more fields (tracking URLs, session IDs, DSA transparency fields) that we don't need.
`twitch-ad-quartile` DATERANGEs are tracking beacons and can be ignored.

**Break length:** each break was 2 × 30 s = 60 s, every 15 minutes, with identical timing and creatives
in `embed` and `picture-by-picture`. My watcher measured ~85 s because ad segments stay in the
28 s window after the break ends. The real length comes from the DATERANGE.

## 3. Player types differ, and that's the opening

- During the same 40 minutes on tarik, **`site` got no video ads**, while `embed` and
  `picture-by-picture` each got three 60 s breaks.
- Instead, `site` received `twitch-maf-ad` DATERANGEs (`PLANNED-DURATION=60`). These are display-ad
  formats that the web player shows *next to* the running video. **They don't touch the video
  segments**, so we can simply ignore them.
- `embed` and `picture-by-picture` behaved identically (same breaks, same creatives), so as backups
  they count as one source, not two.

## 4. What this means for M3 (ad handling)

1. **Primary player type `site`:** it had the fewest video ads in this sample. Keep it configurable.
2. **Detection,** in order of reliability:
   1. `twitch-stitched-ad` DATERANGE, mapped onto segments through `START-DATE` + `DURATION` and the
      segments' `PROGRAM-DATE-TIME`. It arrives early.
   2. Segment title other than `live`.
   3. `twitch-stream-source` value other than `live`.

   `twitch-maf-ad` is not a video ad and is ignored.
3. **Backup swap is plausible:** while one player type was in an ad break, another had live content.
   Open for S2: do both playlists share the same `PROGRAM-DATE-TIME` timeline? That decides whether
   we can splice segment-exact or only approximately.
4. **The overlay countdown** can use `POD-FILLED-DURATION` and needs no guessing.
5. **Fixtures:** 229 playlist snapshots from 4 breaks are recorded locally in `data/s1/`. In M3 they
   get cleaned (URLs, session/ad IDs, tokens) and become test fixtures.

## Not tested yet

- Prerolls: none occurred, even though each watcher started fresh.
- Behaviour while logged in (Turbo/subscriber), and other regions or times of day.
- Whether the segments are fetchable and continuous when switching between player types: S2.
