//! Configuration from the environment (and `.env` during development).

use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context, Result};
use nitch_twitch::AppCredentials;

pub struct Config {
    /// `NITCH_BIND`, default `0.0.0.0:8080`.
    pub bind: SocketAddr,
    /// `NITCH_DATA_DIR`, default `./data`. Holds the SQLite database.
    pub data_dir: PathBuf,
    /// `NITCH_WEB_DIR`, default `./web/dist`. The built single-page app.
    pub web_dir: PathBuf,
    pub twitch: AppCredentials,
}

fn var(name: &str) -> Option<String> {
    std::env::var(name).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

impl Config {
    pub fn from_env() -> Result<Self> {
        // .env is looked up in the working directory and its parents.
        let _ = dotenvy::dotenv();
        Ok(Self {
            bind: var("NITCH_BIND").unwrap_or_else(|| "0.0.0.0:8080".into()).parse().context("NITCH_BIND")?,
            data_dir: var("NITCH_DATA_DIR").unwrap_or_else(|| "data".into()).into(),
            web_dir: var("NITCH_WEB_DIR").unwrap_or_else(|| "web/dist".into()).into(),
            twitch: AppCredentials {
                client_id: var("NITCH_TWITCH_CLIENT_ID").context("NITCH_TWITCH_CLIENT_ID is not set")?,
                client_secret: var("NITCH_TWITCH_CLIENT_SECRET").context("NITCH_TWITCH_CLIENT_SECRET is not set")?,
            },
        })
    }
}
