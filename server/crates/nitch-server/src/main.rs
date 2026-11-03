//! Nitch server: REST/WebSocket API, single view and (later) multiview, serving the web app.

mod api;
mod auth;
mod config;
mod discovery;
mod multiview;
mod play;
mod sessions;
mod store;
mod worker;

use std::sync::Arc;

use anyhow::Result;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

fn main() -> Result<()> {
    // A multiview pipeline in its own process (see worker.rs); logs go to stderr, stdout is
    // the status channel to the server.
    if std::env::args().nth(1).as_deref() == Some(worker::SUBCOMMAND) {
        tracing_subscriber::fmt()
            .with_writer(std::io::stderr)
            .with_ansi(false)
            .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
            .init();
        return worker::run();
    }
    server()
}

#[tokio::main]
async fn server() -> Result<()> {
    tracing_subscriber::fmt()
        // Colors only on a terminal, not in `docker logs` or log files.
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stdout()))
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,nitch_server=debug,tower_http=info")),
        )
        .init();

    let config = config::Config::from_env()?;
    let store = Arc::new(store::Store::open(&config.data_dir.join("nitch.db"))?);
    let tw = nitch_twitch::Twitch::new(config.twitch.clone())?;
    let player = Arc::new(play::Player::new(tw.clone()));
    let multiview =
        multiview::Manager::new(player.clone(), &config.data_dir, config.bind.port(), config.encoder.clone(), config.max_multiview)?;
    let state = Arc::new(api::AppState {
        auth: auth::Auth::load(tw.clone(), store.clone())?,
        store,
        player,
        multiview: multiview.clone(),
        tw,
    });

    // Housekeeping for multiview: stop idle pipelines; retry offline tiles every minute.
    let housekeeping = state.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(10));
        let mut n = 0u64;
        loop {
            tick.tick().await;
            housekeeping.multiview.reap();
            // A crashed pipeline that's still being watched comes back on its own.
            for id in housekeeping.multiview.failed_but_watched() {
                if let Ok(Some(session)) = housekeeping.store.session(&id) {
                    tracing::warn!(%id, "multiview pipeline failed, restarting it");
                    housekeeping.multiview.stop(&id);
                    if let Err(e) = housekeeping.multiview.play(&session).await {
                        tracing::warn!(%id, "restarting the multiview failed: {e:#}");
                    }
                }
            }
            n += 1;
            if n % 6 == 0 {
                for id in housekeeping.multiview.with_offline_tiles() {
                    if let Ok(Some(session)) = housekeeping.store.session(&id) {
                        housekeeping.multiview.reset_offline(&id);
                        if let Err(e) = housekeeping.multiview.apply(&session).await {
                            tracing::warn!(%id, "retrying offline tiles failed: {e:#}");
                        }
                    }
                }
            }
        }
    });

    // Everything that isn't API or HLS is the single-page app; unknown paths get index.html.
    let index = config.web_dir.join("index.html");
    if !index.exists() {
        tracing::warn!(dir = %config.web_dir.display(), "web app not built; only the API is available");
    }
    let spa = ServeDir::new(&config.web_dir).fallback(ServeFile::new(index));
    let app = api::router(state).fallback_service(spa).layer(TraceLayer::new_for_http());

    // Discovery for the TV app; optional (NITCH_MDNS=0 turns it off, and it may fail without multicast).
    let _announcement = match std::env::var("NITCH_MDNS").as_deref() {
        Ok("0") => None,
        _ => discovery::announce(config.bind.port())
            .map_err(|e| tracing::warn!("mDNS announcement unavailable: {e:#}"))
            .ok(),
    };

    let listener = tokio::net::TcpListener::bind(config.bind).await?;
    tracing::info!("listening on http://{}", config.bind);
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
            tracing::info!("shutting down");
        })
        .await?;
    Ok(())
}
