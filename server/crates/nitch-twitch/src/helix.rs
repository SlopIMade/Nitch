//! The official Twitch API (Helix). Every call takes the access token to use, so the caller
//! decides between the user token and an app token.

use anyhow::Result;
use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::{Twitch, Unauthorized};

const HELIX: &str = "https://api.twitch.tv/helix";

#[derive(Debug, Clone, Deserialize)]
pub struct Stream {
    pub user_id: String,
    pub user_login: String,
    pub user_name: String,
    pub game_name: String,
    pub title: String,
    pub viewer_count: u64,
    pub started_at: String,
    /// Template with `{width}` and `{height}` placeholders.
    pub thumbnail_url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SearchChannel {
    pub broadcaster_login: String,
    pub display_name: String,
    pub game_name: String,
    pub is_live: bool,
    pub title: String,
    /// Profile picture.
    pub thumbnail_url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct User {
    pub id: String,
    pub login: String,
    pub display_name: String,
    pub profile_image_url: String,
}

#[derive(Deserialize)]
struct Page<T> {
    data: Vec<T>,
}

impl Twitch {
    async fn helix<T: DeserializeOwned>(&self, token: &str, path: &str, query: &[(&str, &str)]) -> Result<Vec<T>> {
        let resp = self
            .http
            .get(format!("{HELIX}/{path}"))
            .header("Client-Id", &self.app.client_id)
            .bearer_auth(token)
            .query(query)
            .send()
            .await?;
        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(Unauthorized.into());
        }
        Ok(resp.error_for_status()?.json::<Page<T>>().await?.data)
    }

    /// Live streams of channels the user follows (needs `user:read:follows`), most viewers first.
    pub async fn followed_streams(&self, token: &str, user_id: &str) -> Result<Vec<Stream>> {
        self.helix(token, "streams/followed", &[("user_id", user_id), ("first", "100")]).await
    }

    pub async fn search_channels(&self, token: &str, query: &str) -> Result<Vec<SearchChannel>> {
        self.helix(token, "search/channels", &[("query", query), ("first", "30")]).await
    }

    pub async fn users_by_login(&self, token: &str, logins: &[&str]) -> Result<Vec<User>> {
        let query: Vec<(&str, &str)> = logins.iter().map(|l| ("login", *l)).collect();
        self.helix(token, "users", &query).await
    }

    pub async fn users_by_id(&self, token: &str, ids: &[&str]) -> Result<Vec<User>> {
        let query: Vec<(&str, &str)> = ids.iter().map(|i| ("id", *i)).collect();
        self.helix(token, "users", &query).await
    }

    pub async fn streams_by_login(&self, token: &str, logins: &[&str]) -> Result<Vec<Stream>> {
        let query: Vec<(&str, &str)> = logins.iter().map(|l| ("user_login", *l)).collect();
        self.helix(token, "streams", &query).await
    }
}
