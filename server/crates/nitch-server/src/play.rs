//! Single view (D1/D15/D16/D19): the server builds an ad-free playlist per rendition, and clients
//! load the live segments straight from the CDN. During ads the server fetches the same rendition
//! from a chain of backup sources (other player types, each its own Twitch session with its own
//! ad schedule) and splices in the first one that has the real stream at that moment. Only if
//! every source is in an ad does a slate fill the time. Each `POST /api/play` creates a short-lived
//! play session.

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
/// Backup sources in priority order. `embed` got its ads at other times than `site` in S1; the
/// others add independent sessions so that some source is ad-free as often as possible.
pub const BACKUP_PLAYER_TYPES: &[&str] = &["embed", "popout", "autoplay", "frontpage"];
/// After a backup failed to resolve, wait this long before trying it again.
const BACKUP_RETRY: Duration = Duration::from_secs(30);

pub const SLATES: Slates = Slates { ts: "/hls/slate.ts", fmp4_init: "/hls/slate-init.mp4", fmp4: "/hls/slate.m4s" };

/// Options for a play session. Everything but the defaults is for testing ad handling.
#[derive(Debug, Clone)]
pub struct PlayOptions {
    pub player_type: String,
    /// Backup player types in priority order; empty disables backups (ads become slates).
    pub backups: Vec<String>,
    /// Treat 30 s of every 2 minutes as an ad.
    pub simulate_ads: bool,
}

impl PlayOptions {
    pub fn new(player_type: &str) -> Self {
        let backups = BACKUP_PLAYER_TYPES.iter().filter(|p| **p != player_type).map(|p| p.to_string()).collect();
        Self { player_type: player_type.into(), backups, simulate_ads: false }
    }
}

impl Default for PlayOptions {
    fn default() -> Self {
        Self::new(PRIMARY_PLAYER_TYPE)
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

/// One backup source; its master playlist is resolved lazily, at the first ad.
struct Backup {
    player_type: String,
    master: Option<Master>,
    failed: Option<Instant>,
}

struct PlaySession {
    channel: String,
    options: PlayOptions,
    primary: Master,
    backups: Vec<Backup>,
    timeline: Timeline,
    status: PlayStatus,
    /// Start of the current ad break in stream time (the playlist window's start moves on).
    break_from: Option<i64>,
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

    pub fn new_id(&self) -> String {
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
        let backups = options
            .backups
            .iter()
            .map(|p| Backup { player_type: p.clone(), master: None, failed: None })
            .collect();
        let mut sessions = self.sessions.lock().unwrap();
        sessions.retain(|_, s| s.last_access.elapsed() < IDLE);
        sessions.insert(
            id.clone(),
            PlaySession {
                status: PlayStatus { channel: channel.clone(), ad: None },
                channel,
                options,
                primary,
                backups,
                timeline: Timeline::new(),
                break_from: None,
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
        let backups: Vec<Option<MediaPlaylist>> = if primary.has_ads() {
            let fetches = (0..options.backups.len()).map(|k| self.fetch_backup(id, &channel, index, k));
            futures::future::join_all(fetches).await
        } else {
            Vec::new()
        };

        let mut sessions = self.sessions.lock().unwrap();
        let Some(s) = sessions.get_mut(id) else { return Ok(None) };
        let refs: Vec<Option<&MediaPlaylist>> = backups.iter().map(Option::as_ref).collect();
        s.timeline.update(index, &primary, &refs);
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

    /// Media playlist of backup `k` for the same rendition, or `None` if it isn't available now.
    async fn fetch_backup(&self, id: &str, channel: &str, index: usize, k: usize) -> Option<MediaPlaylist> {
        // `Some(None)`: resolved, but this source doesn't offer the rendition (e.g. `autoplay`
        // only has a few); skip it for this rendition instead of resolving it again and again.
        let (known, may_retry, player_type) = self.with(id, |s| {
            let b = s.backups.get(k)?;
            let known = b.master.as_ref().map(|m| matching_variant(m, &s.primary, index));
            Some((known, b.failed.is_none_or(|t| t.elapsed() > BACKUP_RETRY), b.player_type.clone()))
        })?;
        let url = match known {
            Some(Some(u)) => u,
            Some(None) => return None,
            None if may_retry => match self.tw.master_playlist(channel, &player_type).await {
                Ok(master) => {
                    tracing::info!(%channel, %player_type, "backup source resolved");
                    self.with(id, |s| {
                        let url = matching_variant(&master, &s.primary, index);
                        s.backups[k].master = Some(master);
                        url
                    })?
                }
                Err(e) => {
                    tracing::warn!(%channel, %player_type, "backup source unavailable: {e:#}");
                    self.with(id, |s| {
                        s.backups[k].failed = Some(Instant::now());
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
                tracing::info!(%channel, %player_type, %status, "backup playlist rejected, will resolve it again");
                self.with(id, |s| {
                    s.backups[k].master = None;
                    Some(())
                });
                None
            }
            Err(e) => {
                tracing::warn!(%channel, %player_type, "fetching the backup failed: {e:#}");
                None
            }
        }
    }
}

fn update_status(s: &mut PlaySession, primary: &MediaPlaylist) {
    let ad = primary.ad_window().map(|(from, until)| AdStatus {
        from,
        until,
        filled_by: if s.options.backups.is_empty() || s.timeline.recent_slate(3) {
            FilledBy::Slate
        } else {
            FilledBy::Backup
        },
    });
    if let Some(a) = &ad {
        s.break_from = Some(s.break_from.map_or(a.from, |f| f.min(a.from)));
    }
    let before = s.status.ad.as_ref().map(|a| a.filled_by);
    let after = ad.as_ref().map(|a| a.filled_by);
    if before != after {
        match (s.break_from, after) {
            (_, Some(by)) => tracing::info!(channel = %s.channel, ?by, "ad break"),
            // The break is over: report which source filled how much of it.
            (Some(from), None) => {
                s.break_from = None;
                let mut parts: Vec<String> = s
                    .timeline
                    .origins_since(from)
                    .into_iter()
                    .map(|(origin, n)| match origin {
                        Origin::Primary => format!("primary={n}"),
                        Origin::Backup(k) => format!("{}={n}", s.options.backups.get(k).map_or("?", String::as_str)),
                        Origin::Slate => format!("slate={n}"),
                    })
                    .collect();
                parts.sort();
                tracing::info!(channel = %s.channel, filled = %parts.join(" "), "ad break over (segments per source)");
            }
            (None, None) => {}
        }
    }
    s.status.ad = ad;
}
