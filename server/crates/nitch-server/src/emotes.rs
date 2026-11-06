//! Third-party emotes for the chat (D37): 7TV's global set and each channel's set, by name. The
//! chat sends words, not emote positions, for these; the client swaps matching words for images.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use serde::Deserialize;

const GLOBAL_URL: &str = "https://7tv.io/v3/emote-sets/global";
/// A channel's 7TV account by its Twitch user id.
const USER_URL: &str = "https://7tv.io/v3/users/twitch/";
/// Sets change rarely; a channel's new emote shows up within this.
const TTL: Duration = Duration::from_secs(30 * 60);
/// After an error, ask again after this.
const RETRY: Duration = Duration::from_secs(5 * 60);

/// Emote name → 7TV emote id.
pub type EmoteMap = Arc<HashMap<String, String>>;

#[derive(Deserialize)]
struct Set {
    #[serde(default)]
    emotes: Option<Vec<Emote>>,
}

#[derive(Deserialize)]
struct Emote {
    id: String,
    name: String,
}

#[derive(Deserialize)]
struct User {
    emote_set: Option<Set>,
}

pub struct Emotes {
    http: reqwest::Client,
    /// `""` for the global set, else the Twitch user id; with the time it expires.
    cache: Mutex<HashMap<String, (Instant, EmoteMap)>>,
}

fn to_map(set: Option<Set>) -> HashMap<String, String> {
    set.and_then(|s| s.emotes).unwrap_or_default().into_iter().map(|e| (e.name, e.id)).collect()
}

impl Emotes {
    pub fn new() -> Self {
        let http = reqwest::Client::builder().timeout(Duration::from_secs(10)).build().expect("http client");
        Self { http, cache: Mutex::new(HashMap::new()) }
    }

    /// The global set merged with `twitch_id`'s channel set (the channel's names win).
    pub async fn for_channel(&self, twitch_id: &str) -> EmoteMap {
        let global = self.cached("", || async { Ok(to_map(Some(self.get::<Set>(GLOBAL_URL).await?))) }).await;
        let channel = self
            .cached(twitch_id, || async {
                // 404: the channel has no 7TV account, which is common.
                match self.http.get(format!("{USER_URL}{twitch_id}")).send().await?.status() {
                    reqwest::StatusCode::NOT_FOUND => Ok(HashMap::new()),
                    _ => Ok(to_map(self.get::<User>(&format!("{USER_URL}{twitch_id}")).await?.emote_set)),
                }
            })
            .await;
        let mut all = (*global).clone();
        all.extend(channel.iter().map(|(k, v)| (k.clone(), v.clone())));
        Arc::new(all)
    }

    async fn get<T: serde::de::DeserializeOwned>(&self, url: &str) -> Result<T> {
        let resp = self.http.get(url).send().await?;
        if !resp.status().is_success() {
            bail!("7TV returned {}", resp.status());
        }
        Ok(resp.json().await?)
    }

    async fn cached<F, Fut>(&self, key: &str, fetch: F) -> EmoteMap
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<HashMap<String, String>>>,
    {
        if let Some((until, map)) = self.cache.lock().unwrap().get(key) {
            if Instant::now() < *until {
                return map.clone();
            }
        }
        let (ttl, map) = match fetch().await {
            Ok(map) => (TTL, Arc::new(map)),
            Err(e) => {
                tracing::debug!(key, "7TV emotes unavailable: {e:#}");
                // Keep what we had, if anything.
                let old = self.cache.lock().unwrap().get(key).map(|(_, m)| m.clone());
                (RETRY, old.unwrap_or_default())
            }
        };
        self.cache.lock().unwrap().insert(key.to_string(), (Instant::now() + ttl, map.clone()));
        map
    }
}
