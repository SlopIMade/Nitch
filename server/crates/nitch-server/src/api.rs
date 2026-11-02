//! REST API under `/api` and the HLS endpoints under `/hls` (see docs/ARCHITECTURE.md).

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use nitch_twitch::helix::Stream;
use nitch_twitch::playback::Unavailable;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::auth::{Auth, AuthStatus};
use crate::play::{PlayOptions, PlayStatus, Player};

pub struct AppState {
    pub auth: Arc<Auth>,
    pub player: Player,
    pub tw: nitch_twitch::Twitch,
}

type St = State<Arc<AppState>>;

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/auth", get(auth_status).delete(logout))
        .route("/api/auth/device", post(start_login))
        .route("/api/follows/live", get(follows_live))
        .route("/api/search", get(search))
        .route("/api/channels/{login}", get(channel))
        .route("/api/play", post(play))
        .route("/api/play/{id}", get(play_status))
        .route("/hls/slate.ts", get(|| slate("video/mp2t", include_bytes!("../assets/slate.ts"))))
        .route("/hls/slate-init.mp4", get(|| slate("video/mp4", include_bytes!("../assets/slate-init.mp4"))))
        .route("/hls/slate.m4s", get(|| slate("video/iso.segment", include_bytes!("../assets/slate.m4s"))))
        .route("/hls/{id}/master.m3u8", get(hls_master))
        .route("/hls/{id}/{file}", get(hls_media))
        .with_state(state)
}

// ---------- errors ----------

pub enum ApiError {
    NotLoggedIn,
    NotFound(String),
    BadRequest(String),
    Upstream(anyhow::Error),
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        match e.downcast_ref::<Unavailable>() {
            Some(u) => ApiError::NotFound(u.to_string()),
            None => ApiError::Upstream(e),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            ApiError::NotLoggedIn => (StatusCode::UNAUTHORIZED, "not_logged_in", "log in first".to_string()),
            ApiError::NotFound(m) => (StatusCode::NOT_FOUND, "not_found", m),
            ApiError::BadRequest(m) => (StatusCode::BAD_REQUEST, "bad_request", m),
            ApiError::Upstream(e) => {
                tracing::warn!("upstream error: {e:#}");
                (StatusCode::BAD_GATEWAY, "upstream", format!("{e:#}"))
            }
        };
        (status, Json(json!({ "error": code, "message": message }))).into_response()
    }
}

type ApiResult<T> = Result<Json<T>, ApiError>;

// ---------- response types ----------

#[derive(Serialize)]
pub struct LiveStream {
    login: String,
    display_name: String,
    title: String,
    game: String,
    viewers: u64,
    started_at: String,
    thumbnail_url: String,
}

impl From<Stream> for LiveStream {
    fn from(s: Stream) -> Self {
        Self {
            thumbnail_url: s.thumbnail_url.replace("{width}", "640").replace("{height}", "360"),
            login: s.user_login,
            display_name: s.user_name,
            title: s.title,
            game: s.game_name,
            viewers: s.viewer_count,
            started_at: s.started_at,
        }
    }
}

#[derive(Serialize)]
struct SearchResult {
    login: String,
    display_name: String,
    title: String,
    game: String,
    is_live: bool,
    avatar_url: String,
}

#[derive(Serialize)]
struct ChannelInfo {
    login: String,
    display_name: String,
    avatar_url: String,
    live: Option<LiveStream>,
}

// ---------- handlers ----------

async fn health() -> Json<serde_json::Value> {
    Json(json!({ "ok": true, "version": env!("CARGO_PKG_VERSION") }))
}

async fn auth_status(State(s): St) -> Json<AuthStatus> {
    Json(s.auth.status().await)
}

async fn start_login(State(s): St) -> ApiResult<AuthStatus> {
    Ok(Json(s.auth.start_login().await?))
}

async fn logout(State(s): St) -> ApiResult<AuthStatus> {
    s.auth.logout().await?;
    Ok(Json(s.auth.status().await))
}

async fn follows_live(State(s): St) -> ApiResult<Vec<LiveStream>> {
    let tw = &s.tw;
    let streams = s
        .auth
        .call(true, |t| async move { tw.followed_streams(&t.token, t.user_id.as_deref().unwrap_or_default()).await })
        .await?
        .ok_or(ApiError::NotLoggedIn)?;
    Ok(Json(streams.into_iter().map(LiveStream::from).collect()))
}

#[derive(Deserialize)]
struct SearchQuery {
    q: String,
}

async fn search(State(s): St, Query(q): Query<SearchQuery>) -> ApiResult<Vec<SearchResult>> {
    let query = q.q.trim();
    if query.is_empty() {
        return Ok(Json(Vec::new()));
    }
    let tw = &s.tw;
    let mut found = s
        .auth
        .call(false, |t| async move { tw.search_channels(&t.token, query).await })
        .await?
        .unwrap_or_default();
    // Live channels first, keeping Twitch's relevance order otherwise.
    found.sort_by_key(|c| !c.is_live);
    Ok(Json(
        found
            .into_iter()
            .map(|c| SearchResult {
                login: c.broadcaster_login,
                display_name: c.display_name,
                title: c.title,
                game: c.game_name,
                is_live: c.is_live,
                avatar_url: c.thumbnail_url,
            })
            .collect(),
    ))
}

async fn channel(State(s): St, Path(login): Path<String>) -> ApiResult<ChannelInfo> {
    let tw = &s.tw;
    let login = login.to_lowercase();
    let l = login.as_str();
    let (users, streams) = s
        .auth
        .call(false, |t| async move {
            let logins = [l];
            tokio::try_join!(tw.users_by_login(&t.token, &logins), tw.streams_by_login(&t.token, &logins))
        })
        .await?
        .unwrap_or_default();
    let user = users.into_iter().next().ok_or_else(|| ApiError::NotFound(format!("no channel '{login}'")))?;
    Ok(Json(ChannelInfo {
        login: user.login,
        display_name: user.display_name,
        avatar_url: user.profile_image_url,
        live: streams.into_iter().next().map(LiveStream::from),
    }))
}

#[derive(Deserialize)]
struct PlayRequest {
    channel: String,
    /// Testing ad handling: primary player type (default `site`).
    player_type: Option<String>,
    /// Testing: `false` disables the backup source, so ads become slates.
    backup: Option<bool>,
    /// Testing: treat 30 s of every 2 minutes as an ad.
    simulate_ads: Option<bool>,
}

const PLAYER_TYPES: &[&str] = &["site", "embed", "popout", "autoplay", "picture-by-picture", "frontpage"];

async fn play(State(s): St, Json(req): Json<PlayRequest>) -> ApiResult<serde_json::Value> {
    let channel = req.channel.trim();
    if channel.is_empty() || !channel.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(ApiError::BadRequest("invalid channel name".into()));
    }
    let mut options = match req.player_type {
        Some(pt) if !PLAYER_TYPES.contains(&pt.as_str()) => {
            return Err(ApiError::BadRequest(format!("unknown player type '{pt}'")));
        }
        Some(pt) => PlayOptions::new(&pt),
        None => PlayOptions::default(),
    };
    if req.backup == Some(false) {
        options.backups.clear();
    }
    options.simulate_ads = req.simulate_ads.unwrap_or(false);
    let id = s.player.start(channel, options).await?;
    Ok(Json(json!({ "id": id, "hls_url": format!("/hls/{id}/master.m3u8") })))
}

async fn play_status(State(s): St, Path(id): Path<String>) -> ApiResult<PlayStatus> {
    s.player.status(&id).map(Json).ok_or_else(|| ApiError::NotFound("unknown play session".into()))
}

async fn slate(content_type: &'static str, bytes: &'static [u8]) -> Response {
    ([(header::CONTENT_TYPE, content_type), (header::CACHE_CONTROL, "public, max-age=86400")], bytes).into_response()
}

fn m3u8(body: String) -> Response {
    ([(header::CONTENT_TYPE, "application/vnd.apple.mpegurl"), (header::CACHE_CONTROL, "no-cache")], body)
        .into_response()
}

async fn hls_master(State(s): St, Path(id): Path<String>) -> Result<Response, ApiError> {
    let body = s.player.master(&id).ok_or_else(|| ApiError::NotFound("unknown play session".into()))?;
    Ok(m3u8(body))
}

async fn hls_media(State(s): St, Path((id, file)): Path<(String, String)>) -> Result<Response, ApiError> {
    let index: usize = file
        .strip_prefix('v')
        .and_then(|f| f.strip_suffix(".m3u8"))
        .and_then(|n| n.parse().ok())
        .ok_or_else(|| ApiError::NotFound(format!("no such playlist '{file}'")))?;
    let body = s.player.media(&id, index).await?.ok_or_else(|| ApiError::NotFound("unknown play session".into()))?;
    Ok(m3u8(body))
}
