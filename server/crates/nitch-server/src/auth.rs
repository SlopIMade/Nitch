//! The one shared Twitch login (D5): device code login, persistence, automatic refresh, and the
//! choice of token for API calls (user token when logged in, app token otherwise).

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use nitch_twitch::auth::{DeviceCode, Poll, UserToken, now_unix};
use nitch_twitch::{Twitch, is_unauthorized};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::store::Store;

const SCOPES: &[&str] = &["user:read:follows"];
const STORE_KEY: &str = "twitch_login";
/// Refresh the user token when it expires within this many seconds.
const REFRESH_MARGIN: i64 = 300;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInfo {
    pub id: String,
    pub login: String,
    pub display_name: String,
    pub profile_image_url: String,
}

#[derive(Clone, Serialize, Deserialize)]
struct Login {
    token: UserToken,
    user: UserInfo,
}

enum State {
    LoggedOut,
    Pending { code: DeviceCode, expires_at: i64 },
    LoggedIn(Login),
}

/// What clients see (`GET /api/auth`).
#[derive(Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum AuthStatus {
    LoggedOut,
    Pending { user_code: String, verification_uri: String, expires_at: i64 },
    LoggedIn { user: UserInfo },
}

pub struct Auth {
    tw: Twitch,
    store: Arc<Store>,
    state: Mutex<State>,
    app_token: Mutex<Option<(String, i64)>>,
    /// Fired whenever the login state changes (for WebSocket clients later).
    pub changed: tokio::sync::watch::Sender<u64>,
}

/// The token an API call should use.
pub struct ApiToken {
    pub token: String,
    /// Set when it's the user's token.
    pub user_id: Option<String>,
}

impl Auth {
    pub fn load(tw: Twitch, store: Arc<Store>) -> Result<Arc<Self>> {
        let state = match store.get::<Login>(STORE_KEY)? {
            Some(login) => {
                tracing::info!(user = %login.user.login, "restored Twitch login");
                State::LoggedIn(login)
            }
            None => State::LoggedOut,
        };
        Ok(Arc::new(Self {
            tw,
            store,
            state: Mutex::new(state),
            app_token: Mutex::new(None),
            changed: tokio::sync::watch::Sender::new(0),
        }))
    }

    fn notify(&self) {
        self.changed.send_modify(|n| *n += 1);
    }

    pub async fn status(&self) -> AuthStatus {
        let mut state = self.state.lock().await;
        if let State::Pending { expires_at, .. } = &*state {
            if *expires_at <= now_unix() {
                *state = State::LoggedOut;
            }
        }
        match &*state {
            State::LoggedOut => AuthStatus::LoggedOut,
            State::Pending { code, expires_at } => AuthStatus::Pending {
                user_code: code.user_code.clone(),
                verification_uri: code.verification_uri.clone(),
                expires_at: *expires_at,
            },
            State::LoggedIn(login) => AuthStatus::LoggedIn { user: login.user.clone() },
        }
    }

    /// Starts a device login (or returns the one already pending) and polls it in the background.
    pub async fn start_login(self: &Arc<Self>) -> Result<AuthStatus> {
        if matches!(self.status().await, AuthStatus::Pending { .. } | AuthStatus::LoggedIn { .. }) {
            return Ok(self.status().await);
        }
        let code = self.tw.start_device_login(SCOPES).await?;
        let expires_at = now_unix() + code.expires_in;
        *self.state.lock().await = State::Pending { code: code.clone(), expires_at };
        self.notify();
        tracing::info!(user_code = %code.user_code, "device login started");
        let this = self.clone();
        tokio::spawn(async move { this.poll(code, expires_at).await });
        Ok(self.status().await)
    }

    async fn poll(self: Arc<Self>, code: DeviceCode, expires_at: i64) {
        let mut interval = code.interval.max(1);
        while now_unix() < expires_at {
            tokio::time::sleep(Duration::from_secs(interval)).await;
            // Stop if this login was cancelled or replaced.
            let current = matches!(&*self.state.lock().await,
                State::Pending { code: c, .. } if c.device_code == code.device_code);
            if !current {
                return;
            }
            match self.tw.poll_device_login(&code, SCOPES).await {
                Ok(Poll::Pending) => tracing::debug!("device login: still waiting for confirmation"),
                Ok(Poll::SlowDown) => {
                    tracing::debug!("device login: slow down");
                    interval += 5
                }
                Ok(Poll::Done(token)) => {
                    match self.finish_login(token).await {
                        Ok(user) => tracing::info!(user = %user.login, "logged in"),
                        Err(e) => {
                            tracing::error!("login finished but loading the user failed: {e:#}");
                            *self.state.lock().await = State::LoggedOut;
                        }
                    }
                    self.notify();
                    return;
                }
                Ok(Poll::Expired(reason)) => {
                    tracing::info!(%reason, "device login ended");
                    break;
                }
                Err(e) => tracing::warn!("polling device login failed: {e:#}"),
            }
        }
        let mut state = self.state.lock().await;
        if matches!(&*state, State::Pending { code: c, .. } if c.device_code == code.device_code) {
            *state = State::LoggedOut;
            drop(state);
            self.notify();
        }
    }

    async fn finish_login(&self, token: UserToken) -> Result<UserInfo> {
        let validated = self
            .tw
            .validate(&token.access_token)
            .await?
            .ok_or_else(|| anyhow::anyhow!("new token failed validation"))?;
        let user = self
            .tw
            .users_by_id(&token.access_token, &[&validated.user_id])
            .await?
            .into_iter()
            .next()
            .map(|u| UserInfo { id: u.id, login: u.login, display_name: u.display_name, profile_image_url: u.profile_image_url })
            .ok_or_else(|| anyhow::anyhow!("user {} not found", validated.user_id))?;
        let login = Login { token, user: user.clone() };
        self.store.set(STORE_KEY, &login)?;
        *self.state.lock().await = State::LoggedIn(login);
        Ok(user)
    }

    pub async fn logout(&self) -> Result<()> {
        let previous = std::mem::replace(&mut *self.state.lock().await, State::LoggedOut);
        self.store.delete(STORE_KEY)?;
        if let State::LoggedIn(login) = previous {
            if let Err(e) = self.tw.revoke(&login.token.access_token).await {
                tracing::warn!("revoking the token failed: {e:#}");
            }
            tracing::info!(user = %login.user.login, "logged out");
        }
        self.notify();
        Ok(())
    }

    /// The user's token, refreshed if it's about to expire (or if `force`). `None` when logged out.
    async fn user_token(&self, force: bool) -> Result<Option<ApiToken>> {
        let mut state = self.state.lock().await;
        let State::LoggedIn(login) = &mut *state else { return Ok(None) };
        if force || login.token.expires_at - now_unix() < REFRESH_MARGIN {
            match self.tw.refresh(&login.token).await {
                Ok(token) => {
                    login.token = token;
                    self.store.set(STORE_KEY, &*login)?;
                    tracing::debug!("refreshed the user token");
                }
                Err(e) if is_unauthorized(&e) => {
                    // The user revoked access (or changed their password): back to logged out.
                    tracing::warn!("refresh token rejected, logging out");
                    *state = State::LoggedOut;
                    self.store.delete(STORE_KEY)?;
                    drop(state);
                    self.notify();
                    return Ok(None);
                }
                Err(e) => return Err(e),
            }
        }
        Ok(Some(ApiToken { token: login.token.access_token.clone(), user_id: Some(login.user.id.clone()) }))
    }

    async fn app_token(&self, force: bool) -> Result<ApiToken> {
        let mut cached = self.app_token.lock().await;
        if force || cached.as_ref().is_none_or(|(_, exp)| exp - now_unix() < REFRESH_MARGIN) {
            *cached = Some(self.tw.app_token().await?);
        }
        Ok(ApiToken { token: cached.as_ref().unwrap().0.clone(), user_id: None })
    }

    async fn token(&self, need_user: bool, force: bool) -> Result<Option<ApiToken>> {
        match self.user_token(force).await? {
            Some(t) => Ok(Some(t)),
            None if need_user => Ok(None),
            None => Ok(Some(self.app_token(force).await?)),
        }
    }

    /// Runs a Helix call with the right token, refreshing and retrying once if Twitch rejects it.
    /// `Ok(None)` means the call needs a user and nobody is logged in.
    pub async fn call<T, F, Fut>(&self, need_user: bool, f: F) -> Result<Option<T>>
    where
        F: Fn(ApiToken) -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        let Some(token) = self.token(need_user, false).await? else { return Ok(None) };
        match f(token).await {
            Err(e) if is_unauthorized(&e) => {
                let Some(token) = self.token(need_user, true).await? else { return Ok(None) };
                f(token).await.map(Some)
            }
            other => other.map(Some),
        }
    }
}
