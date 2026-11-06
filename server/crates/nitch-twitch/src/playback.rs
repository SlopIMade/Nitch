//! Anonymous playback: GQL playback access token + usher master playlist (spikes S1/S2).
//! Unofficial and isolated here, because Twitch can change it without notice.

use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use reqwest::StatusCode;
use serde_json::{Value, json};
use url::Url;

use crate::Twitch;

/// Client ID of Twitch's own web player. Playback tokens are issued for it; it is only used for
/// the anonymous token request, never for the user's login.
const WEB_PLAYER_CLIENT_ID: &str = "kimne78kx3ncx6brgo4mv6wki5h1ko";
const GQL_URL: &str = "https://gql.twitch.tv/gql";
pub(crate) const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                                     (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36";

/// The query the web player sends as `PlaybackAccessToken_Template`.
const TOKEN_QUERY: &str = r#"query PlaybackAccessToken_Template($login: String!, $isLive: Boolean!, $vodID: ID!, $isVod: Boolean!, $playerType: String!, $platform: String!) {
  streamPlaybackAccessToken(channelName: $login, params: {platform: $platform, playerBackend: "mediaplayer", playerType: $playerType}) @include(if: $isLive) { value signature __typename }
  videoPlaybackAccessToken(id: $vodID, params: {platform: $platform, playerBackend: "mediaplayer", playerType: $playerType}) @include(if: $isVod) { value signature __typename }
}"#;

/// `type@platform` (e.g. `autoplay@android`) → the player type and platform; plain types are
/// `web`.
fn split_player_type(player_type: &str) -> (&str, &str) {
    player_type.split_once('@').unwrap_or((player_type, "web"))
}

/// Player type with the fewest video ads in S1; `embed` is the backup source (M3).
pub const PRIMARY_PLAYER_TYPE: &str = "site";

pub struct PlaybackToken {
    pub value: String,
    pub signature: String,
    /// The platform it was issued for; the usher request must name the same.
    pub platform: String,
}

/// Why a master playlist couldn't be fetched.
#[derive(Debug)]
pub enum Unavailable {
    Offline,
    NotFound,
}

impl std::fmt::Display for Unavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Unavailable::Offline => "channel is offline",
            Unavailable::NotFound => "channel not found",
        })
    }
}

impl std::error::Error for Unavailable {}

fn nanos() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos()
}

impl Twitch {
    /// `player_type` may name a platform: `autoplay@android` (default `web`).
    pub async fn playback_token(&self, login: &str, player_type: &str) -> Result<PlaybackToken> {
        let (player_type, platform) = split_player_type(player_type);
        let body = json!({
            "operationName": "PlaybackAccessToken_Template",
            "query": TOKEN_QUERY,
            "variables": {
                "isLive": true, "login": login.to_lowercase(), "isVod": false, "vodID": "",
                "playerType": player_type, "platform": platform,
            },
        });
        let resp: Value = self
            .http
            .post(GQL_URL)
            .header("Client-ID", WEB_PLAYER_CLIENT_ID)
            .header("X-Device-Id", format!("{:032x}", nanos()))
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
            return Err(Unavailable::NotFound.into());
        }
        Ok(PlaybackToken {
            value: t["value"].as_str().context("token value missing")?.to_owned(),
            signature: t["signature"].as_str().context("token signature missing")?.to_owned(),
            platform: platform.to_owned(),
        })
    }

    /// Master playlist (H.264 renditions only) for a live channel.
    pub async fn master_playlist(&self, login: &str, player_type: &str) -> Result<nitch_hls_master::Master> {
        let token = self.playback_token(login, player_type).await?;
        let mut url = Url::parse(&format!("https://usher.ttvnw.net/api/channel/hls/{}.m3u8", login.to_lowercase()))?;
        url.query_pairs_mut()
            .append_pair("allow_source", "true")
            .append_pair("allow_audio_only", "true")
            .append_pair("fast_bread", "true")
            .append_pair("p", &(nanos() % 10_000_000).to_string())
            .append_pair("platform", &token.platform)
            .append_pair("player_backend", "mediaplayer")
            .append_pair("playlist_include_framerate", "true")
            .append_pair("reassignments_supported", "true")
            .append_pair("supported_codecs", "h264")
            .append_pair("sig", &token.signature)
            .append_pair("token", &token.value);
        let resp = self.http.get(url.clone()).send().await?;
        match resp.status() {
            StatusCode::NOT_FOUND => return Err(Unavailable::Offline.into()),
            s if !s.is_success() => bail!("usher returned {s}"),
            _ => {}
        }
        let text = resp.text().await?;
        nitch_hls_master::parse_master(&text, &url)
    }

    /// Fetches a playlist or other text resource; returns status and body.
    pub async fn get_text(&self, url: &Url) -> Result<(StatusCode, String)> {
        let resp = self.http.get(url.clone()).send().await?;
        let status = resp.status();
        Ok((status, resp.text().await?))
    }
}

/// Re-export so callers don't need to depend on nitch-hls just for the master type.
pub use nitch_hls as nitch_hls_master;
