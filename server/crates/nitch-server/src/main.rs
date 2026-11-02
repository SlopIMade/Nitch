//! Nitch server: REST/WebSocket API, single view and (later) multiview, serving the web app.

mod api;
mod auth;
mod config;
mod play;
mod sessions;
mod store;

use std::sync::Arc;

use anyhow::Result;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
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
    let state = Arc::new(api::AppState {
        auth: auth::Auth::load(tw.clone(), store.clone())?,
        store,
        player: play::Player::new(tw.clone()),
        tw,
    });

    // Everything that isn't API or HLS is the single-page app; unknown paths get index.html.
    let index = config.web_dir.join("index.html");
    if !index.exists() {
        tracing::warn!(dir = %config.web_dir.display(), "web app not built; only the API is available");
    }
    let spa = ServeDir::new(&config.web_dir).fallback(ServeFile::new(index));
    let app = api::router(state).fallback_service(spa).layer(TraceLayer::new_for_http());

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
