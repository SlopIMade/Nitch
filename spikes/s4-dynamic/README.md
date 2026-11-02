# Spike S4 — clock-paced multiview with runtime changes

Throwaway tool (see [ROADMAP → M1](../../docs/ROADMAP.md#m1--spikes-throwaway-code-is-fine)). It builds the
multiview pipeline the way M4 will:
- every tile is a `fallbacksrc` with a slate
- live sources pace the output
- tiles sit on the 60×60 grid

A scripted scenario then changes layout, replaces tiles, breaks one and switches audio while it runs.

What we learned is in [FINDINGS.md](FINDINGS.md). `twitch.rs` and `hls.rs` are copies from S3.

## Usage (in WSL)

```bash
cd <repo>/spikes/s4-dynamic
# six live channels: 4 to start, 1 to swap in, 1 for the late replacement
cargo run -q -- --channels a,b,c,d,e,f --frames /tmp/frames   # scripted scenario, ~100 s
cargo run -q -- --channels a,b,c,d,e,f --hold 60               # steady 2×2, CPU per thread
```

| Option | Default | Meaning |
|---|---|---|
| `--q` | `480p` | Input rendition per tile |
| `--preset` | `veryfast` | x264 speed preset |
| `--bg` | `tiny` | Live background that paces the compositor: `tiny` (16×16) or `full` (1080p) |
| `--frames` | `<out>/frames` | Where frame grabs from the scenario go |
| `--out` | `/tmp/nitch-s4` | HLS output |
