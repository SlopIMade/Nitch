//! OAuth for the registered Nitch app: device code login (TV shows a code, phone confirms),
//! refresh, validation, revocation, and app access tokens for calls without a user.

use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::{Twitch, Unauthorized};

const DEVICE_URL: &str = "https://id.twitch.tv/oauth2/device";
const TOKEN_URL: &str = "https://id.twitch.tv/oauth2/token";
const VALIDATE_URL: &str = "https://id.twitch.tv/oauth2/validate";
const REVOKE_URL: &str = "https://id.twitch.tv/oauth2/revoke";
const DEVICE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";

pub fn now_unix() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

#[derive(Debug, Clone, Deserialize)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: i64,
    pub interval: u64,
}

/// A user token as stored by the server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserToken {
    pub access_token: String,
    pub refresh_token: String,
    /// Unix seconds.
    pub expires_at: i64,
    pub scopes: Vec<String>,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: i64,
    #[serde(default)]
    scope: Vec<String>,
}

impl TokenResponse {
    fn into_user_token(self, previous_refresh: Option<&str>) -> Result<UserToken> {
        let refresh_token = self
            .refresh_token
            .or_else(|| previous_refresh.map(str::to_string))
            .context("token response without refresh_token")?;
        Ok(UserToken {
            access_token: self.access_token,
            refresh_token,
            expires_at: now_unix() + self.expires_in,
            scopes: self.scope,
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Validated {
    pub login: String,
    pub user_id: String,
    pub expires_in: i64,
}

pub enum Poll {
    /// The user hasn't confirmed yet.
    Pending,
    /// Polling too fast; wait longer.
    SlowDown,
    Done(UserToken),
    /// The code expired or was rejected; start again.
    Expired(String),
}

#[derive(Deserialize)]
struct ErrorBody {
    #[serde(default)]
    message: String,
}

impl Twitch {
    pub async fn start_device_login(&self, scopes: &[&str]) -> Result<DeviceCode> {
        let resp = self
            .http
            .post(DEVICE_URL)
            .form(&[("client_id", self.app.client_id.as_str()), ("scopes", &scopes.join(" "))])
            .send()
            .await?;
        if !resp.status().is_success() {
            bail!("device login start failed: {} {}", resp.status(), resp.text().await.unwrap_or_default());
        }
        Ok(resp.json().await?)
    }

    pub async fn poll_device_login(&self, code: &DeviceCode, scopes: &[&str]) -> Result<Poll> {
        let scopes = scopes.join(" ");
        let resp = self
            .http
            .post(TOKEN_URL)
            .form(&[
                ("client_id", self.app.client_id.as_str()),
                ("client_secret", self.app.client_secret.as_str()),
                ("device_code", code.device_code.as_str()),
                ("grant_type", DEVICE_GRANT),
                ("scopes", scopes.as_str()),
            ])
            .send()
            .await?;
        if resp.status().is_success() {
            let token: TokenResponse = resp.json().await?;
            return Ok(Poll::Done(token.into_user_token(None)?));
        }
        let message = resp.json::<ErrorBody>().await.map(|b| b.message).unwrap_or_default();
        Ok(match message.as_str() {
            "authorization_pending" => Poll::Pending,
            "slow_down" => Poll::SlowDown,
            _ => Poll::Expired(message),
        })
    }

    pub async fn refresh(&self, token: &UserToken) -> Result<UserToken> {
        let resp = self
            .http
            .post(TOKEN_URL)
            .form(&[
                ("client_id", self.app.client_id.as_str()),
                ("client_secret", self.app.client_secret.as_str()),
                ("grant_type", "refresh_token"),
                ("refresh_token", token.refresh_token.as_str()),
            ])
            .send()
            .await?;
        if resp.status() == reqwest::StatusCode::BAD_REQUEST || resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(Unauthorized.into());
        }
        let resp = resp.error_for_status()?;
        resp.json::<TokenResponse>().await?.into_user_token(Some(&token.refresh_token))
    }

    /// `None` if the token is no longer valid.
    pub async fn validate(&self, access_token: &str) -> Result<Option<Validated>> {
        let resp = self.http.get(VALIDATE_URL).bearer_auth(access_token).send().await?;
        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Ok(None);
        }
        Ok(Some(resp.error_for_status()?.json().await?))
    }

    pub async fn revoke(&self, token: &str) -> Result<()> {
        self.http
            .post(REVOKE_URL)
            .form(&[("client_id", self.app.client_id.as_str()), ("token", token)])
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }

    /// App access token (client credentials) for Helix calls that don't need a user.
    pub async fn app_token(&self) -> Result<(String, i64)> {
        #[derive(Deserialize)]
        struct AppToken {
            access_token: String,
            expires_in: i64,
        }
        let t: AppToken = self
            .http
            .post(TOKEN_URL)
            .form(&[
                ("client_id", self.app.client_id.as_str()),
                ("client_secret", self.app.client_secret.as_str()),
                ("grant_type", "client_credentials"),
            ])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok((t.access_token, now_unix() + t.expires_in))
    }
}
