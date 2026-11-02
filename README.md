# Nitch

A self-hosted Twitch viewer for desktop browsers and Android TV.

A small server runs at home (Docker on Linux). It logs in to Twitch once, serves the web app,
deals with ad breaks, and can merge several live streams into a single video, so even a weak
TV stick can show a 2×2 multiview.

> **Status:** planning — no code yet.

## Docs

- [Architecture](docs/ARCHITECTURE.md) — components, playback paths, multiview pipeline, ad handling, API
- [Roadmap](docs/ROADMAP.md) — milestones, open questions, risks
- [Decisions](docs/DECISIONS.md) — what we chose and why
- [Development setup](docs/DEVELOPMENT.md) — tools for this PC and checks for the target server

## Planned layout

```
server/       Rust workspace (tokio + axum + GStreamer)
web/          React + TypeScript + Vite single-page app
android-tv/   Kotlin WebView shell for Android TV
deploy/       Dockerfile, compose.yaml
docs/         Planning and design documents
```

## Disclaimer

Nitch is an unofficial personal project and is not affiliated with Twitch. Avoiding ads goes
against Twitch's Terms of Service; use at your own risk.
