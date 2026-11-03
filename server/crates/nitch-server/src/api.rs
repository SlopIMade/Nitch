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
use crate::multiview::{self, MultiviewInfo};
use crate::play::{PlayOptions, PlayStatus, Player};
use crate::sessions::{self, Session, SessionInput};
use crate::store::Store;

pub struct AppState {
    pub auth: Arc<Auth>,
    pub chat: Arc<crate::chat::ChatHub>,
    pub player: Arc<Player>,
    pub multiview: Arc<multiview::Manager>,
    pub store: Arc<Store>,
    pub tw: nitch_twitch::Twitch,
    pub started: std::time::Instant,
    /// Events for every WebSocket client (e.g. `session_updated`), as JSON.
    pub events: tokio::sync::broadcast::Sender<String>,
}

impl AppState {
    fn event(&self, value: serde_json::Value) {
        let _ = self.events.send(value.to_string());
    }
}

type St = State<Arc<AppState>>;

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/system", get(system))
        .route("/api/ws", get(websocket))
        .route("/api/auth", get(auth_status).delete(logout))
        .route("/api/auth/device", post(start_login))
        .route("/api/follows/live", get(follows_live))
        .route("/api/search", get(search))
        .route("/api/channels/{login}", get(channel))
        .route("/api/play", post(play))
        .route("/api/play/{id}", get(play_status).delete(stop_play))
        .route("/api/sessions", get(list_sessions).post(create_session))
        .route("/api/sessions/{id}", get(get_session).patch(update_session).delete(delete_session))
        .route("/api/sessions/{id}/play", get(multiview_status).post(play_session))
        .route("/hls/mv/{id}/{*path}", get(multiview_file))
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
    /// A resource limit was reached (e.g. running multiviews).
    Busy(String),
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
            ApiError::Busy(m) => (StatusCode::SERVICE_UNAVAILABLE, "busy", m),
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

/// Diagnostics for the settings screen.
async fn system(State(s): St) -> Json<serde_json::Value> {
    Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "uptime_seconds": s.started.elapsed().as_secs(),
        // Wall-clock rate per monotonic second (D26); 1.0 on a well-behaved host.
        "clock_rate": nitch_media::clock::measured_rate(),
        "multiview": s.multiview.diagnostics(),
        "play_sessions": s.player.diagnostics(),
        "chat_channels": s.chat.channels(),
    }))
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

/// The player left this stream: end the play session now instead of waiting for it to idle out.
async fn stop_play(State(s): St, Path(id): Path<String>) -> StatusCode {
    s.player.remove(&id);
    StatusCode::NO_CONTENT
}

// ---------- multiview sessions (D17) ----------

async fn list_sessions(State(s): St) -> ApiResult<Vec<Session>> {
    Ok(Json(s.store.sessions()?))
}

async fn get_session(State(s): St, Path(id): Path<String>) -> ApiResult<Session> {
    s.store.session(&id)?.map(Json).ok_or_else(|| ApiError::NotFound("no such session".into()))
}

async fn create_session(State(s): St, Json(input): Json<SessionInput>) -> Result<Response, ApiError> {
    let name = sessions::validate_name(input.name.as_deref().unwrap_or("Multiview")).map_err(ApiError::BadRequest)?;
    let tiles = sessions::validate_tiles(input.tiles.unwrap_or_default()).map_err(ApiError::BadRequest)?;
    let now = nitch_twitch::auth::now_unix();
    let session = Session { id: s.player.new_id(), name, tiles, created_at: now, updated_at: now };
    s.store.save_session(&session)?;
    tracing::info!(id = %session.id, name = %session.name, tiles = session.tiles.len(), "session created");
    s.event(json!({ "type": "session_updated", "id": session.id }));
    Ok((StatusCode::CREATED, Json(session)).into_response())
}

async fn update_session(State(s): St, Path(id): Path<String>, Json(input): Json<SessionInput>) -> ApiResult<Session> {
    let mut session = s.store.session(&id)?.ok_or_else(|| ApiError::NotFound("no such session".into()))?;
    if let Some(name) = input.name {
        session.name = sessions::validate_name(&name).map_err(ApiError::BadRequest)?;
    }
    if let Some(tiles) = input.tiles {
        session.tiles = sessions::validate_tiles(tiles).map_err(ApiError::BadRequest)?;
    }
    session.updated_at = nitch_twitch::auth::now_unix();
    s.store.save_session(&session)?;
    // A running pipeline follows the edit live, and so do other devices showing the session.
    s.multiview.apply(&session).await?;
    s.event(json!({ "type": "session_updated", "id": session.id }));
    Ok(Json(session))
}

async fn delete_session(State(s): St, Path(id): Path<String>) -> Result<StatusCode, ApiError> {
    s.multiview.stop(&id);
    if s.store.delete_session(&id)? {
        tracing::info!(%id, "session deleted");
        s.event(json!({ "type": "session_updated", "id": id }));
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound("no such session".into()))
    }
}

/// Starts the session's multiview (or joins the running one).
async fn play_session(State(s): St, Path(id): Path<String>) -> ApiResult<MultiviewInfo> {
    let session = s.store.session(&id)?.ok_or_else(|| ApiError::NotFound("no such session".into()))?;
    if session.tiles.is_empty() {
        return Err(ApiError::BadRequest("the session has no tiles".into()));
    }
    let info = s.multiview.play(&session).await.map_err(|e| {
        if e.to_string().contains("already running") { ApiError::Busy(format!("{e:#}")) } else { ApiError::Upstream(e) }
    })?;
    Ok(Json(info))
}

/// State of a running multiview: tiles per slot with their state (`live`, `loading`, …).
async fn multiview_status(State(s): St, Path(id): Path<String>) -> ApiResult<MultiviewInfo> {
    s.multiview.info(&id).map(Json).ok_or_else(|| ApiError::NotFound("not running".into()))
}

/// Playlists and segments of a running multiview, from its output directory.
async fn multiview_file(State(s): St, Path((id, path)): Path<(String, String)>) -> Result<Response, ApiError> {
    let safe = |p: &str| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c)) && p != "..";
    let parts: Vec<&str> = path.split('/').collect();
    if !safe(&id) || parts.len() > 2 || !parts.iter().all(|p| safe(p)) {
        return Err(ApiError::NotFound("no such file".into()));
    }
    let file = s.multiview.dir(&id).join(&path);
    let bytes = tokio::fs::read(&file).await.map_err(|_| ApiError::NotFound("no such file".into()))?;
    let (content_type, cache) = match file.extension().and_then(|e| e.to_str()) {
        Some("m3u8") => {
            s.multiview.touch(&id);
            ("application/vnd.apple.mpegurl", "no-cache")
        }
        Some("mp4") => ("video/mp4", "no-cache"),
        _ => ("video/iso.segment", "max-age=60"),
    };
    Ok(([(header::CONTENT_TYPE, content_type), (header::CACHE_CONTROL, cache)], bytes).into_response())
}

// ---------- WebSocket: chat (M6), later more events ----------

/// Most channels one client may follow at once (a full multiview).
const MAX_CHAT_CHANNELS: usize = 9;

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientEvent {
    /// The chats this client shows now (replaces the previous set).
    ChatSubscribe { channels: Vec<String> },
}

#[derive(Serialize)]
struct ChatEvent<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    #[serde(flatten)]
    msg: &'a crate::chat::ChatMessage,
}

async fn websocket(State(s): St, ws: axum::extract::ws::WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |socket| websocket_client(socket, s))
}

async fn websocket_client(socket: axum::extract::ws::WebSocket, s: Arc<AppState>) {
    use axum::extract::ws::Message;
    use futures::{SinkExt, StreamExt};
    use std::collections::HashSet;

    let (mut tx, mut rx) = socket.split();
    let mut messages = s.chat.messages();
    let mut events = s.events.subscribe();
    let mut subscribed: HashSet<String> = HashSet::new();
    loop {
        tokio::select! {
            incoming = rx.next() => match incoming {
                Some(Ok(Message::Text(text))) => {
                    let Ok(ClientEvent::ChatSubscribe { channels }) = serde_json::from_str(&text) else { continue };
                    let wanted: HashSet<String> =
                        channels.iter().map(|c| c.to_lowercase()).take(MAX_CHAT_CHANNELS).collect();
                    for c in subscribed.difference(&wanted) {
                        s.chat.unsubscribe(c);
                    }
                    for c in wanted.difference(&subscribed) {
                        s.chat.subscribe(c);
                        // What was said before (if the server already reads that chat).
                        let history = s.chat.history(c);
                        let event = json!({ "type": "chat_history", "channel": c, "messages": history });
                        if tx.send(Message::text(event.to_string())).await.is_err() {
                            return;
                        }
                    }
                    subscribed = wanted;
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                _ => {}
            },
            msg = messages.recv() => match msg {
                Ok(m) if subscribed.contains(&m.channel) => {
                    let Ok(json) = serde_json::to_string(&ChatEvent { kind: "chat", msg: &m }) else { continue };
                    if tx.send(Message::text(json)).await.is_err() {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                _ => {}
            },
            event = events.recv() => match event {
                Ok(json) => {
                    if tx.send(Message::text(json)).await.is_err() {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                Err(_) => {}
            },
        }
    }
    for c in &subscribed {
        s.chat.unsubscribe(c);
    }
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
