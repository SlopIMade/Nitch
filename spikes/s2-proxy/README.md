# Spike S2 — playlist proxy and player behaviour

Throwaway tool (see [ROADMAP → M1](../../docs/ROADMAP.md#m1--spikes-throwaway-code-is-fine)). It answers:

- How does hls.js react when the proxy drops ad segments?
- Can clients load segments straight from the Twitch CDN (CORS)?
- Do the `site` and `embed` playlists share a timeline, so backup segments can replace ads exactly?

What we learned is in [FINDINGS.md](FINDINGS.md). `twitch.rs` and `hls.rs` are copies from S1.

## Usage (in WSL)

```bash
cd <repo>/spikes/s2-proxy
cargo run -q -- align <channel>        # site vs embed timeline + CORS check
cargo run -q -- serve --port 8787      # then open http://localhost:8787 on Windows
```

On the test page you can choose:

| Setting | Options |
|---|---|
| Player type | `site`, `embed`, `picture-by-picture` |
| Rendition | e.g. `720p` |
| Mode | `passthrough` (unchanged), `drop` (remove real ads), `sim` (also drop a fake 30 s "ad" every 2 min) |
| Segments | `direct` (from the CDN) or `proxy` (through the server) |

The page shows buffer, latency, stalls and an event log. Everything is also available on
`window.s2` in the browser console.
