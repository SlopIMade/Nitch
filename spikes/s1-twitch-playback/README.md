# Spike S1 — Twitch playback and ad markers

Throwaway tool (see [ROADMAP → M1](../../docs/ROADMAP.md#m1--spikes-throwaway-code-is-fine)). It answers:

- Can we get live HLS playlists from Twitch without logging in?
- What do ad breaks look like in the playlists, and do other player types get ads at the same time?

What we learned is in [FINDINGS.md](FINDINGS.md).

## Usage (in WSL)

```bash
cd <repo>/spikes/s1-twitch-playback
cargo run -q -- info <channel>    # token flags + renditions
cargo run -q -- probe <channel>   # ad markers right now, per player type
cargo run -q -- watch <channel> --minutes 40 --record <repo>/data/s1
```

`watch` options:

- `--rendition 720p` follows a rendition other than the source (substring match).
- `--player-type embed` uses a different player type (default `site`).

For each run, `watch` writes the following to `<record>/<channel>-<time>/`:

- `events.jsonl`: every event as one JSON line (also printed to the console)
- `NNNN-seq<N>.m3u8`: media playlists from 3 refreshes before each ad break until 3 refreshes after it

**Raw recordings stay local** (`data/` is git-ignored): they contain session IDs and signed URLs.
Test data for the repo gets cleaned of those in M3.
