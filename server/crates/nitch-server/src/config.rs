//! Configuration from the environment (and `.env` during development).

use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use nitch_media::Encoder;
use nitch_twitch::AppCredentials;

pub struct Config {
    /// `NITCH_BIND`, default `0.0.0.0:8080`.
    pub bind: SocketAddr,
    /// `NITCH_DATA_DIR`, default `./data`. Holds the SQLite database and multiview output.
    pub data_dir: PathBuf,
    /// `NITCH_WEB_DIR`, default `./web/dist`. The built single-page app.
    pub web_dir: PathBuf,
    pub twitch: AppCredentials,
    /// `NITCH_ENCODER`: `x264` (default) or `va` (VA-API, falls back to x264 if unavailable).
    /// `NITCH_X264_PRESET`, default `veryfast`.
    pub encoder: Encoder,
    /// `NITCH_MAX_MULTIVIEW`: multiview pipelines running at once, default 2 (~2 cores each in
    /// software, S3/S4).
    pub max_multiview: usize,
}

fn var(name: &str) -> Option<String> {
    std::env::var(name).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

impl Config {
    pub fn from_env() -> Result<Self> {
        // .env is looked up in the working directory and its parents.
        let _ = dotenvy::dotenv();
        let encoder = match var("NITCH_ENCODER").as_deref().unwrap_or("x264") {
            "x264" => Encoder::X264 { preset: var("NITCH_X264_PRESET").unwrap_or_else(|| "veryfast".into()) },
            "va" => Encoder::Va,
            other => bail!("NITCH_ENCODER must be x264 or va, not '{other}'"),
        };
        Ok(Self {
            bind: var("NITCH_BIND").unwrap_or_else(|| "0.0.0.0:8080".into()).parse().context("NITCH_BIND")?,
            data_dir: var("NITCH_DATA_DIR").unwrap_or_else(|| "data".into()).into(),
            web_dir: var("NITCH_WEB_DIR").unwrap_or_else(|| "web/dist".into()).into(),
            twitch: AppCredentials {
                client_id: var("NITCH_TWITCH_CLIENT_ID").context("NITCH_TWITCH_CLIENT_ID is not set")?,
                client_secret: var("NITCH_TWITCH_CLIENT_SECRET").context("NITCH_TWITCH_CLIENT_SECRET is not set")?,
            },
            encoder,
            max_multiview: var("NITCH_MAX_MULTIVIEW").map(|v| v.parse()).transpose().context("NITCH_MAX_MULTIVIEW")?.unwrap_or(2),
        })
    }
}
