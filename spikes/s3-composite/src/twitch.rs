//! Anonymous playback access token (GQL) and usher master playlist.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use reqwest::{Client, StatusCode};
use serde_json::{Value, json};
use url::Url;

use crate::hls::{self, Master};

/// Client ID of Twitch's own web player. GQL playback tokens are issued for it.
const WEB_CLIENT_ID: &str = "kimne78kx3ncx6brgo4mv6wki5h1ko";
const GQL_URL: &str = "https://gql.twitch.tv/gql";
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                          (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36";

/// The query the web player sends as `PlaybackAccessToken_Template`.
const TOKEN_QUERY: &str = r#"query PlaybackAccessToken_Template($login: String!, $isLive: Boolean!, $vodID: ID!, $isVod: Boolean!, $playerType: String!) {
  streamPlaybackAccessToken(channelName: $login, params: {platform: "web", playerBackend: "mediaplayer", playerType: $playerType}) @include(if: $isLive) { value signature __typename }
  videoPlaybackAccessToken(id: $vodID, params: {platform: "web", playerBackend: "mediaplayer", playerType: $playerType}) @include(if: $isVod) { value signature __typename }
}"#;

pub struct Twitch {
    http: Client,
    device_id: String,
}

pub struct Token {
    pub value: String,
    pub signature: String,
}

impl Token {
    /// Ad-related fields of the token. Leaves out user_ip and other identifying fields.
    pub fn flags(&self) -> String {
        let v: Value = serde_json::from_str(&self.value).unwrap_or(Value::Null);
        let keys = [
            "player_type", "server_ads", "show_ads", "hide_ads", "adblock", "turbo", "subscriber",
            "geoblock_reason",
        ];
        keys.iter()
            .filter_map(|k| v.get(k).map(|x| format!("{k}={x}")))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

fn nanos() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos()
}

impl Twitch {
    pub fn new() -> Result<Self> {
        let http = Client::builder()
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(10))
            .build()?;
        Ok(Self { http, device_id: format!("{:032x}", nanos()) })
    }

    pub fn http(&self) -> &Client {
        &self.http
    }

    pub async fn playback_token(&self, login: &str, player_type: &str) -> Result<Token> {
        let body = json!({
            "operationName": "PlaybackAccessToken_Template",
            "query": TOKEN_QUERY,
            "variables": {
                "isLive": true, "login": login.to_lowercase(), "isVod": false, "vodID": "",
                "playerType": player_type,
            },
        });
        let resp: Value = self
            .http
            .post(GQL_URL)
            .header("Client-ID", WEB_CLIENT_ID)
            .header("X-Device-Id", &self.device_id)
            .json(&body)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        if let Some(errors) = resp.get("errors") {
            bail!("GQL errors: {errors}");
        }
        let t = &resp["data"]["streamPlaybackAccessToken"];
        if t.is_null() {
            bail!("no playback token for '{login}' (does the channel exist?)");
        }
        Ok(Token {
            value: t["value"].as_str().context("token value missing")?.to_owned(),
            signature: t["signature"].as_str().context("token signature missing")?.to_owned(),
        })
    }

    pub async fn master_playlist(&self, login: &str, token: &Token) -> Result<Master> {
        let mut url = Url::parse(&format!(
            "https://usher.ttvnw.net/api/channel/hls/{}.m3u8",
            login.to_lowercase()
        ))?;
        url.query_pairs_mut()
            .append_pair("allow_source", "true")
            .append_pair("allow_audio_only", "true")
            .append_pair("fast_bread", "true")
            .append_pair("p", &(nanos() % 10_000_000).to_string())
            .append_pair("platform", "web")
            .append_pair("player_backend", "mediaplayer")
            .append_pair("playlist_include_framerate", "true")
            .append_pair("reassignments_supported", "true")
            .append_pair("supported_codecs", "h264")
            .append_pair("sig", &token.signature)
            .append_pair("token", &token.value);
        let (status, text) = self.get_text(&url).await?;
        if status == StatusCode::NOT_FOUND {
            bail!("'{login}' is offline");
        }
        if !status.is_success() {
            bail!("usher returned {status}: {}", text.chars().take(200).collect::<String>());
        }
        hls::parse_master(&text, &url)
    }

    pub async fn get_text(&self, url: &Url) -> Result<(StatusCode, String)> {
        let resp = self.http.get(url.clone()).send().await?;
        let status = resp.status();
        Ok((status, resp.text().await?))
    }
}
