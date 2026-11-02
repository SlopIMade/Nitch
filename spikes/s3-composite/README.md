# Spike S3 — 2×2 composite, software encoding

Throwaway tool (see [ROADMAP → M1](../../docs/ROADMAP.md#m1--spikes-throwaway-code-is-fine)). It merges
1–4 live Twitch channels into a 1080p grid, encodes it with x264 into HLS, and measures CPU use.

What we learned is in [FINDINGS.md](FINDINGS.md). `twitch.rs` and `hls.rs` are copies from S2.

## Usage (in WSL)

```bash
cd <repo>/spikes/s3-composite
cargo run -q -- --channels a,b,c,d --seconds 75
```

| Option | Default | Meaning |
|---|---|---|
| `--q` | `480p` | Input rendition per tile (substring match) |
| `--preset` | `veryfast` | x264 speed preset |
| `--fps` | `30` | Output frame rate |
| `--bitrate` | `6000` | Output video bitrate in kbit/s |
| `--align-tiles` | `true` | Shift a late tile's timeline onto the output position |
| `--out` | `/tmp/nitch-s3` | Where the HLS output goes |

It prints CPU use every 5 s and a `SUMMARY` line at the end. To look at a frame:

```bash
cd /tmp/nitch-s3 && gst-launch-1.0 -q filesrc location=seg00020.ts ! tsdemux ! h264parse ! avdec_h264 \
  ! videoconvert ! pngenc snapshot=true ! filesink location=frame.png
```
