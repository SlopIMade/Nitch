# Nitch

A self-hosted Twitch viewer for desktop browsers and Android TV.

A small server runs at home (Docker on Linux). It logs in to Twitch once, serves the web app,
deals with ad breaks, and can merge several live streams into a single video, so even a weak
TV stick can show a 2×2 multiview.

> **Status:** M1–M7 done on the development PC. Deploying to the target VM (M8) and the first
> Android TV test are still open.
>
> What works:
> - **Login and browsing:** log in via phone (device code), followed live channels, channel search
> - **Single view:** with ads replaced by a chain of ad-free backup sources (an "Ad break" slate only
>   when every source is in an ad at once)
> - **Multiview:** sessions on a 60×60 grid, merged on the server, saved on the server, edited live,
>   with one audio track per tile
> - **Chat overlay:** read-only, for the focused stream or several streams combined; movable,
>   resizable and zoomable
> - **TV mode:** for the Android TV app (remote navigation, Back key, server discovery)

## Quick start

1. Copy `.env.example` to `.env` and fill in your Twitch app's Client ID and secret.
2. Start it:

   ```bash
   docker compose -f deploy/compose.yaml up -d --build
   ```

3. Open `http://<server>:8080`.

Full guide (Twitch app, options, hardware encoding, TV app): [docs/INSTALL.md](docs/INSTALL.md).
Development setup: [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md).

## Docs

- [Architecture](docs/ARCHITECTURE.md) — components, playback paths, multiview pipeline, ad handling, API
- [Roadmap](docs/ROADMAP.md) — milestones, open questions, risks
- [Decisions](docs/DECISIONS.md) — what we chose and why
- [Install](docs/INSTALL.md) — Twitch app, configuration, Docker, the TV app
- [Development setup](docs/DEVELOPMENT.md) — tools for this PC and checks for the target server

## Layout

```
server/       Rust workspace: nitch-server (API, web app, multiview manager, chat),
              nitch-twitch, nitch-hls (ads, timeline), nitch-media (GStreamer pipelines)
web/          React + TypeScript + Vite single-page app
deploy/       Dockerfile, compose.yaml
spikes/       Throwaway experiments from M1, each with FINDINGS.md
scripts/      WSL setup and environment check
docs/         Architecture, roadmap, decisions, development setup
android-tv/   Kotlin WebView shell for Android TV (build in Android Studio)
```

## Disclaimer

Nitch is an unofficial personal project and is not affiliated with Twitch. Avoiding ads goes
against Twitch's Terms of Service; use at your own risk.
