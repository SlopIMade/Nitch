//! Single view (D1/D15/D16): the server builds an ad-free playlist per rendition, and clients load
//! the live segments straight from the CDN. During ads the server fetches the same rendition
//! from the backup player type and splices its segments in; if that's in an ad too, slates fill
//! the time. Each `POST /api/play` creates a short-lived play session.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use nitch_hls::Master;
use nitch_hls::media::{MediaPlaylist, parse_media};
use nitch_hls::timeline::{Origin, Slates, Timeline};
use nitch_twitch::Twitch;
use nitch_twitch::playback::PRIMARY_PLAYER_TYPE;
use serde::Serialize;
use url::Url;

/// Play sessions nobody fetched for this long are dropped.
const IDLE: Duration = Duration::from_secs(10 * 60);
/// Player type for the backup source (S1: got its ads at other times than `site`).
pub const BACKUP_PLAYER_TYPE: &str = "embed";
/// After the backup failed to resolve, wait this long before trying again.
const BACKUP_RETRY: Duration = Duration::from_secs(30);

pub const SLATES: Slates = Slates { ts: "/hls/slate.ts", fmp4_init: "/hls/slate-init.mp4", fmp4: "/hls/slate.m4s" };

/// Options for a play session. Everything but the channel is for testing ad handling.
#[derive(Debug, Clone)]
pub struct PlayOptions {
    pub player_type: String,
    pub backup: bool,
    /// Treat 30 s of every 2 minutes as an ad.
    pub simulate_ads: bool,
}

impl Default for PlayOptions {
    fn default() -> Self {
        Self { player_type: PRIMARY_PLAYER_TYPE.into(), backup: true, simulate_ads: false }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FilledBy {
    Backup,
    Slate,
}

/// What clients see about ads (`GET /api/play/{id}`).
#[derive(Debug, Clone, Serialize, Default)]
pub struct PlayStatus {
    pub channel: String,
    /// Set while the primary playlist contains or announces an ad.
    pub ad: Option<AdStatus>,
}

/// The ad break in stream time (Unix ms). The player runs behind the live edge, so it compares
/// these with its own program date time to show the badge at the right moment.
#[derive(Debug, Clone, Serialize)]
pub struct AdStatus {
    pub from: i64,
    /// `None` while the end isn't known yet.
    pub until: Option<i64>,
    pub filled_by: FilledBy,
}

struct PlaySession {
    channel: String,
    options: PlayOptions,
    primary: Master,
    backup: Option<Master>,
    backup_failed: Option<Instant>,
    timeline: Timeline,
    status: PlayStatus,
    last_access: Instant,
}

pub struct Player {
    tw: Twitch,
    sessions: Mutex<HashMap<String, PlaySession>>,
    counter: AtomicU64,
}

/// The backup's rendition matching the primary's: same name, else same resolution.
fn matching_variant(master: &Master, primary: &Master, index: usize) -> Option<Url> {
    let p = primary.variants.get(index)?;
    master
        .variants
        .iter()
        .find(|v| v.name == p.name)
        .or_else(|| master.variants.iter().find(|v| v.resolution.is_some() && v.resolution == p.resolution))
        .map(|v| v.uri.clone())
}

impl Player {
    pub fn new(tw: Twitch) -> Self {
        Self { tw, sessions: Mutex::new(HashMap::new()), counter: AtomicU64::new(0) }
    }

    fn new_id(&self) -> String {
        let n = self.counter.fetch_add(1, Ordering::Relaxed);
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        format!("{:x}{n:x}", t & 0xffff_ffff_ffff)
    }

    /// Resolves the channel and returns the new play session's id.
    pub async fn start(&self, channel: &str, options: PlayOptions) -> Result<String> {
        let primary = self.tw.master_playlist(channel, &options.player_type).await?;
        let id = self.new_id();
        tracing::info!(%channel, %id, ?options, renditions = primary.variants.len(), "play session started");
        let channel = channel.to_lowercase();
        let mut sessions = self.sessions.lock().unwrap();
        sessions.retain(|_, s| s.last_access.elapsed() < IDLE);
        sessions.insert(
            id.clone(),
            PlaySession {
                status: PlayStatus { channel: channel.clone(), ad: None },
                channel,
                options,
                primary,
                backup: None,
                backup_failed: None,
                timeline: Timeline::new(),
                last_access: Instant::now(),
            },
        );
        Ok(id)
    }

    /// `None` if the session doesn't exist (anymore).
    pub fn master(&self, id: &str) -> Option<String> {
        let mut sessions = self.sessions.lock().unwrap();
        let s = sessions.get_mut(id)?;
        s.last_access = Instant::now();
        Some(nitch_hls::rewrite_master(&s.primary, |i| format!("v{i}.m3u8")))
    }

    pub fn status(&self, id: &str) -> Option<PlayStatus> {
        self.sessions.lock().unwrap().get(id).map(|s| s.status.clone())
    }

    /// The ad-free media playlist of rendition `index`.
    pub async fn media(&self, id: &str, index: usize) -> Result<Option<String>> {
        let Some((channel, options, url)) = self.with(id, |s| {
            Some((s.channel.clone(), s.options.clone(), s.primary.variants.get(index)?.uri.clone()))
        }) else {
            return Ok(None);
        };

        let mut primary = self.fetch_primary(id, &channel, &options, index, url).await?;
        if options.simulate_ads {
            primary.simulate_ads(120, 30);
        }
        let backup = if options.backup && primary.has_ads() {
            self.fetch_backup(id, &channel, index).await
        } else {
            None
        };

        let mut sessions = self.sessions.lock().unwrap();
        let Some(s) = sessions.get_mut(id) else { return Ok(None) };
        s.timeline.update(index, &primary, backup.as_ref());
        update_status(s, &primary);
        Ok(Some(s.timeline.render(index, &SLATES)))
    }

    fn with<T>(&self, id: &str, f: impl FnOnce(&mut PlaySession) -> Option<T>) -> Option<T> {
        let mut sessions = self.sessions.lock().unwrap();
        let s = sessions.get_mut(id)?;
        s.last_access = Instant::now();
        f(s)
    }

    /// Primary media playlist; re-resolves the channel once if Twitch's URL expired.
    async fn fetch_primary(
        &self,
        id: &str,
        channel: &str,
        options: &PlayOptions,
        index: usize,
        url: Url,
    ) -> Result<MediaPlaylist> {
        let (status, text) = self.tw.get_text(&url).await?;
        if status.is_success() {
            return parse_media(&text, &url);
        }
        tracing::info!(%channel, %status, "media playlist rejected, resolving the channel again");
        let master = self.tw.master_playlist(channel, &options.player_type).await?;
        let url = master.variants.get(index).context("rendition disappeared")?.uri.clone();
        self.with(id, |s| {
            s.primary = master;
            Some(())
        });
        let (status, text) = self.tw.get_text(&url).await?;
        anyhow::ensure!(status.is_success(), "media playlist returned {status}");
        parse_media(&text, &url)
    }

    /// Backup media playlist for the same rendition, or `None` if it isn't available right now.
    async fn fetch_backup(&self, id: &str, channel: &str, index: usize) -> Option<MediaPlaylist> {
        let (known, may_retry) = self.with(id, |s| {
            let known = s.backup.as_ref().and_then(|b| matching_variant(b, &s.primary, index));
            Some((known, s.backup_failed.is_none_or(|t| t.elapsed() > BACKUP_RETRY)))
        })?;
        let url = match known {
            Some(u) => u,
            None if may_retry => match self.tw.master_playlist(channel, BACKUP_PLAYER_TYPE).await {
                Ok(master) => {
                    tracing::info!(%channel, "backup source resolved");
                    self.with(id, |s| {
                        let url = matching_variant(&master, &s.primary, index);
                        s.backup = Some(master);
                        url
                    })?
                }
                Err(e) => {
                    tracing::warn!(%channel, "backup source unavailable: {e:#}");
                    self.with(id, |s| {
                        s.backup_failed = Some(Instant::now());
                        Some(())
                    });
                    return None;
                }
            },
            None => return None,
        };
        match self.tw.get_text(&url).await {
            Ok((status, text)) if status.is_success() => parse_media(&text, &url).ok(),
            Ok((status, _)) => {
                tracing::info!(%channel, %status, "backup playlist rejected, will resolve it again");
                self.with(id, |s| {
                    s.backup = None;
                    Some(())
                });
                None
            }
            Err(e) => {
                tracing::warn!(%channel, "fetching the backup failed: {e:#}");
                None
            }
        }
    }
}

fn update_status(s: &mut PlaySession, primary: &MediaPlaylist) {
    let ad = primary.ad_window().map(|(from, until)| AdStatus {
        from,
        until,
        filled_by: if !s.options.backup || s.timeline.recent_slate(3) { FilledBy::Slate } else { FilledBy::Backup },
    });
    let before = s.status.ad.as_ref().map(|a| a.filled_by);
    let after = ad.as_ref().map(|a| a.filled_by);
    if before != after {
        match after {
            Some(by) => tracing::info!(channel = %s.channel, ?by, "ad break"),
            None => tracing::info!(channel = %s.channel, last = ?s.timeline.last_origin().unwrap_or(Origin::Primary), "ad break over"),
        }
    }
    s.status.ad = ad;
}
