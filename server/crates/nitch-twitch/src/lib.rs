//! Twitch clients for Nitch:
//! - [`auth`]: OAuth device code login, token refresh/validation/revocation, app tokens
//! - [`helix`]: the official API (followed streams, search, users, streams)
//! - [`playback`]: anonymous playback access token + usher master playlist (unofficial GQL)

pub mod auth;
pub mod helix;
pub mod playback;

use std::time::Duration;

/// Returned (inside `anyhow::Error`) when Twitch rejects a token; callers refresh and retry.
#[derive(Debug)]
pub struct Unauthorized;

impl std::fmt::Display for Unauthorized {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Twitch rejected the token")
    }
}

impl std::error::Error for Unauthorized {}

pub fn is_unauthorized(e: &anyhow::Error) -> bool {
    e.downcast_ref::<Unauthorized>().is_some()
}

/// Credentials of the registered Twitch app (from the environment).
#[derive(Clone)]
pub struct AppCredentials {
    pub client_id: String,
    pub client_secret: String,
}

/// One HTTP client for everything Twitch; cheap to clone.
#[derive(Clone)]
pub struct Twitch {
    http: reqwest::Client,
    app: AppCredentials,
}

impl Twitch {
    pub fn new(app: AppCredentials) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(playback::USER_AGENT)
            .timeout(Duration::from_secs(10))
            .build()?;
        Ok(Self { http, app })
    }

    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    pub fn client_id(&self) -> &str {
        &self.app.client_id
    }
}
