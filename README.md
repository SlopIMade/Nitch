# Nitch

A self-hosted Twitch viewer for desktop browsers and Android TV.

A small server runs at home (Docker on Linux). It logs in to Twitch once, serves the web app,
deals with ad breaks, and can merge several live streams into a single video, so even a weak
TV stick can show a 2×2 multiview.

> **Status:** M2 in progress. Working so far:
> - log in via phone (device code), followed live channels, channel search
> - single view in the browser, with a mini player that keeps playing while you browse
>
> Spikes for ads and multiview are done (see `spikes/*/FINDINGS.md`).

## Quick start

1. Copy `.env.example` to `.env` and fill in your Twitch app's Client ID and secret.
2. Start it:

   ```bash
   docker compose -f deploy/compose.yaml up -d --build
   ```

3. Open `http://<server>:8080`.

Development setup: [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md).

## Docs

- [Architecture](docs/ARCHITECTURE.md) — components, playback paths, multiview pipeline, ad handling, API
- [Roadmap](docs/ROADMAP.md) — milestones, open questions, risks
- [Decisions](docs/DECISIONS.md) — what we chose and why
- [Development setup](docs/DEVELOPMENT.md) — tools for this PC and checks for the target server

## Layout

```
server/       Rust workspace: nitch-server (API, web app), nitch-twitch, nitch-hls
web/          React + TypeScript + Vite single-page app
deploy/       Dockerfile, compose.yaml
spikes/       Throwaway experiments from M1, each with FINDINGS.md
scripts/      WSL setup and environment check
docs/         Architecture, roadmap, decisions, development setup
android-tv/   Kotlin WebView shell for Android TV (M5, not started)
```

## Disclaimer

Nitch is an unofficial personal project and is not affiliated with Twitch. Avoiding ads goes
against Twitch's Terms of Service; use at your own risk.
