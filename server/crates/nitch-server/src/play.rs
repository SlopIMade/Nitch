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
use nitch_twitch::playback::{PRIMARY_PLAYER_TYPE, Unavailable};
use serde::Serialize;
use url::Url;

/// Play sessions nobody fetched for this long are dropped.
/// A play session nobody fetched for this long is dropped. Players refresh the playlist every
/// few seconds, multiview tiles likewise, so only abandoned sessions get this old.
const IDLE: Duration = Duration::from_secs(45);
/// Backup sources in priority order. `embed` got its ads at other times than `site` in S1; the
/// others add independent sessions so that some source is ad-free as often as possible.
pub const BACKUP_PLAYER_TYPES: &[&str] = &["embed", "popout", "autoplay", "frontpage"];
/// After Twitch reported the channel offline, wait this long before asking again.
const OFFLINE_BACKOFF: Duration = Duration::from_secs(15);
/// After a backup failed to resolve, wait this long before trying it again.
const BACKUP_RETRY: Duration = Duration::from_secs(30);

// Pre-warming (D35): every new Twitch session starts with a preroll (all player types alike), and
// it runs from the first playlist fetch, whether anyone keeps fetching or not. So the server opens
// sessions for the followed live channels ahead of time; one whose preroll is over plays at once.
/// Channels kept warm (the followed live ones, most viewers first).
pub const MAX_WARM: usize = 20;
/// A warm session is checked for ads this long after its first fetch (prerolls ran ~60 s).
const WARM_CHECK_AFTER: Duration = Duration::from_secs(70);
/// A replacement is opened when the newest warm session is this old ...
const WARM_RENEW_AFTER: Duration = Duration::from_secs(10 * 60);
/// ... and a warm session is no longer handed out after this (the playback token lasts 20 min).
const WARM_MAX_AGE: Duration = Duration::from_secs(15 * 60);
/// New warm sessions per round, so a long follow list doesn't hit Twitch all at once.
const WARM_OPENS_PER_TICK: usize = 3;

/// A session opened ahead of time for one channel.
struct Warm {
    master: Master,
    opened: Instant,
    /// Checked after its preroll: no ad segments.
    ready: bool,
}

pub const SLATES: Slates = Slates { ts: "/hls/slate.ts", fmp4_init: "/hls/slate-init.mp4", fmp4: "/hls/slate.m4s" };

/// Options for a play session. Everything but the defaults is for testing ad handling.
#[derive(Debug, Clone)]
pub struct PlayOptions {
    pub player_type: String,
    /// Backup player types in priority order; empty disables backups (ads become slates).
    pub backups: Vec<String>,
    /// Treat 30 s of every 2 minutes as an ad.
    pub simulate_ads: bool,
    /// Fill ad time nobody can fill with slates (browser player). Multiview tiles turn this
    /// off: their playlist pauses instead and fallbacksrc bridges the gap.
    pub slates: bool,
}

impl PlayOptions {
    pub fn new(player_type: &str) -> Self {
        let backups = BACKUP_PLAYER_TYPES.iter().filter(|p| **p != player_type).map(|p| p.to_string()).collect();
        Self { player_type: player_type.into(), backups, simulate_ads: false, slates: true }
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
    /// No source is ad-free and slates are off: the stream pauses (multiview tiles).
    Nothing,
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
    /// Set when Twitch reported the channel offline; requests until then fail fast.
    offline_until: Option<Instant>,
    last_access: Instant,
}

pub struct Player {
    tw: Twitch,
    sessions: Mutex<HashMap<String, PlaySession>>,
    counter: AtomicU64,
    /// Warm sessions per channel (lowercase), and the channels to keep warm.
    warm: Mutex<HashMap<String, Vec<Warm>>>,
    warm_channels: Mutex<Vec<String>>,
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

#[derive(Debug, Clone, Copy)]
pub struct VideoRendition {
    pub index: usize,
    pub height: u32,
    pub fps: u32,
    pub bandwidth: u64,
}

/// Twitch names renditions like `720p60`, `480p`, `1080p60 (source)`; no number means 30.
fn fps_from_name(name: &str) -> u32 {
    name.split_once('p')
        .map(|(_, rest)| rest.chars().take_while(char::is_ascii_digit).collect::<String>())
        .and_then(|n| n.parse().ok())
        .unwrap_or(30)
}

/// The cheapest rendition that still looks right in a tile `target` pixels tall: the lowest one
/// reaching 80 % of that (a slight upscale isn't visible), else the highest. The output runs at
/// 30 fps, so 30 fps beats 60 fps of the same height (half the decoding).
pub fn choose_rendition(videos: &[VideoRendition], target: u32) -> Option<usize> {
    let enough = |v: &&VideoRendition| v.height * 5 >= target * 4;
    let cost = |v: &&VideoRendition| (v.height, v.fps > 30, v.bandwidth);
    videos
        .iter()
        .filter(enough)
        .min_by_key(cost)
        .or_else(|| {
            let top = videos.iter().map(|v| v.height).max()?;
            videos.iter().filter(|v| v.height == top).min_by_key(cost)
        })
        .map(|v| v.index)
}

impl Player {
    pub fn new(tw: Twitch) -> Self {
        Self {
            tw,
            sessions: Mutex::new(HashMap::new()),
            counter: AtomicU64::new(0),
            warm: Mutex::new(HashMap::new()),
            warm_channels: Mutex::new(Vec::new()),
        }
    }

    /// The channels to keep warm, in priority order (at most [`MAX_WARM`] are used).
    pub fn set_warm_channels(&self, channels: Vec<String>) {
        let channels: Vec<String> = channels.into_iter().map(|c| c.to_lowercase()).take(MAX_WARM).collect();
        self.warm.lock().unwrap().retain(|c, _| channels.contains(c));
        *self.warm_channels.lock().unwrap() = channels;
    }

    /// One round of pre-warming (every few seconds): drops old warm sessions, checks the ones
    /// whose preroll should be over, and opens new ones where none is young enough.
    pub async fn warm_tick(&self) {
        let lowest = |m: &Master| m.variants.iter().min_by_key(|v| v.bandwidth).map(|v| v.uri.clone());
        let to_check: Vec<(String, Instant, Url)> = {
            let mut warm = self.warm.lock().unwrap();
            for list in warm.values_mut() {
                list.retain(|w| w.opened.elapsed() < WARM_MAX_AGE);
            }
            warm.iter()
                .flat_map(|(c, list)| {
                    list.iter()
                        .filter(|w| !w.ready && w.opened.elapsed() >= WARM_CHECK_AFTER)
                        .filter_map(|w| Some((c.clone(), w.opened, lowest(&w.master)?)))
                })
                .collect()
        };
        for (channel, opened, url) in to_check {
            let result = match self.tw.get_text(&url).await {
                Ok((status, text)) if status.is_success() => parse_media(&text, &url).ok().map(|pl| !pl.segments.iter().any(|s| s.ad)),
                _ => None,
            };
            let mut warm = self.warm.lock().unwrap();
            let Some(list) = warm.get_mut(&channel) else { continue };
            match result {
                Some(true) => {
                    if let Some(w) = list.iter_mut().find(|w| w.opened == opened) {
                        w.ready = true;
                        tracing::debug!(%channel, "warm session ready (preroll over)");
                    }
                }
                // Still in its preroll: check again next round.
                Some(false) => {}
                // Rejected (expired, offline): drop it.
                None => list.retain(|w| w.opened != opened),
            }
        }

        let channels = self.warm_channels.lock().unwrap().clone();
        let due: Vec<String> = {
            let warm = self.warm.lock().unwrap();
            channels
                .into_iter()
                .filter(|c| warm.get(c).is_none_or(|list| list.iter().all(|w| w.opened.elapsed() >= WARM_RENEW_AFTER)))
                .take(WARM_OPENS_PER_TICK)
                .collect()
        };
        for channel in due {
            let opened = async {
                let master = self.tw.master_playlist(&channel, PRIMARY_PLAYER_TYPE).await?;
                // The first fetch starts the preroll's clock.
                let url = lowest(&master).context("no renditions")?;
                self.tw.get_text(&url).await?;
                anyhow::Ok(master)
            };
            match opened.await {
                Ok(master) => {
                    let mut warm = self.warm.lock().unwrap();
                    warm.entry(channel).or_default().push(Warm { master, opened: Instant::now(), ready: false });
                }
                Err(e) => tracing::debug!(%channel, "warm session not opened: {e:#}"),
            }
        }
    }

    /// A warm session of `channel` whose preroll is over, if there is one (it's used up).
    fn take_warm(&self, channel: &str) -> Option<Master> {
        let mut warm = self.warm.lock().unwrap();
        let list = warm.get_mut(&channel.to_lowercase())?;
        // The youngest ready one lasts longest.
        let i = list
            .iter()
            .enumerate()
            .filter(|(_, w)| w.ready && w.opened.elapsed() < WARM_MAX_AGE)
            .min_by_key(|(_, w)| w.opened.elapsed())?
            .0;
        Some(list.remove(i).master)
    }

    /// For diagnostics: warm sessions per channel (`ready` / all).
    pub fn warm_status(&self) -> serde_json::Value {
        let warm = self.warm.lock().unwrap();
        let channels = self.warm_channels.lock().unwrap();
        channels
            .iter()
            .map(|c| {
                let list = warm.get(c);
                let ready = list.map_or(0, |l| l.iter().filter(|w| w.ready).count());
                (c.clone(), serde_json::json!(format!("{ready}/{}", list.map_or(0, Vec::len))))
            })
            .collect::<serde_json::Map<_, _>>()
            .into()
    }

    pub fn new_id(&self) -> String {
        let n = self.counter.fetch_add(1, Ordering::Relaxed);
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        format!("{:x}{n:x}", t & 0xffff_ffff_ffff)
    }

    /// Resolves the channel and returns the new play session's id.
    pub async fn start(&self, channel: &str, options: PlayOptions) -> Result<String> {
        let warm = if options.player_type == PRIMARY_PLAYER_TYPE { self.take_warm(channel) } else { None };
        let prewarmed = warm.is_some();
        let primary = match warm {
            Some(master) => master,
            None => self.tw.master_playlist(channel, &options.player_type).await?,
        };
        let id = self.new_id();
        tracing::info!(%channel, %id, ?options, renditions = primary.variants.len(), prewarmed, "play session started");
        let channel = channel.to_lowercase();
        let slates = options.slates;
        let backups = options
            .backups
            .iter()
            .map(|p| Backup { player_type: p.clone(), master: None, failed: None })
            .collect();
        let mut sessions = self.sessions.lock().unwrap();
        sessions.insert(
            id.clone(),
            PlaySession {
                status: PlayStatus { channel: channel.clone(), ad: None },
                channel,
                options,
                primary,
                backups,
                timeline: Timeline::with_slates(slates),
                break_from: None,
                offline_until: None,
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

    /// The rendition to use for a multiview tile whose picture is `height` pixels tall (see
    /// [`choose_rendition`]).
    pub fn pick_rendition(&self, id: &str, height: u32) -> Option<usize> {
        let sessions = self.sessions.lock().unwrap();
        let videos: Vec<VideoRendition> = sessions
            .get(id)?
            .primary
            .variants
            .iter()
            .enumerate()
            .filter_map(|(index, v)| {
                Some(VideoRendition {
                    index,
                    height: v.resolution.as_deref()?.split_once('x')?.1.parse().ok()?,
                    fps: fps_from_name(&v.name),
                    bandwidth: v.bandwidth,
                })
            })
            .collect();
        choose_rendition(&videos, height)
    }

    /// For diagnostics: active play sessions (channel, ad status).
    pub fn diagnostics(&self) -> Vec<serde_json::Value> {
        self.sessions
            .lock()
            .unwrap()
            .iter()
            .map(|(id, s)| {
                serde_json::json!({
                    "id": id,
                    "channel": s.channel,
                    "idle_seconds": s.last_access.elapsed().as_secs(),
                    "slates": s.options.slates,
                    "ad": s.status.ad,
                })
            })
            .collect()
    }

    /// Drops abandoned play sessions (housekeeping).
    pub fn reap(&self) {
        self.sessions.lock().unwrap().retain(|id, s| {
            let keep = s.last_access.elapsed() < IDLE;
            if !keep {
                tracing::info!(%id, channel = %s.channel, "play session ended (idle)");
            }
            keep
        });
    }

    /// Ends a play session (a player that left it, a multiview tile that was removed).
    pub fn remove(&self, id: &str) {
        self.sessions.lock().unwrap().remove(id);
    }

    /// The ad-free media playlist of rendition `index`.
    pub async fn media(&self, id: &str, index: usize) -> Result<Option<String>> {
        let Some((channel, options, url)) = self.with(id, |s| {
            Some((s.channel.clone(), s.options.clone(), s.primary.variants.get(index)?.uri.clone()))
        }) else {
            return Ok(None);
        };

        let mut primary = self.fetch_primary(id, &channel, &options, index, url).await?;
        tracing::trace!(
            %channel,
            index,
            segments = primary.segments.len(),
            ads = primary.segments.iter().filter(|s| s.ad).count(),
            pdts = ?primary.segments.iter().map(|s| (s.pdt.unwrap_or(0) / 1000 % 3600, s.ad)).collect::<Vec<_>>(),
            "primary playlist"
        );
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
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
        s.timeline.update_at(index, &primary, &refs, Some(now));
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
        // The channel went offline a moment ago: don't ask Twitch again on every retry.
        if self.with(id, |s| s.offline_until.filter(|t| Instant::now() < *t)).is_some() {
            return Err(Unavailable::Offline.into());
        }
        let (status, text) = self.tw.get_text(&url).await?;
        if status.is_success() {
            return parse_media(&text, &url);
        }
        tracing::info!(%channel, %status, "media playlist rejected, resolving the channel again");
        // A warm session avoids the new session's preroll here too.
        let warm = if options.player_type == PRIMARY_PLAYER_TYPE { self.take_warm(channel) } else { None };
        let resolved = match warm {
            Some(m) => Ok(m),
            None => self.tw.master_playlist(channel, &options.player_type).await,
        };
        let master = match resolved {
            Ok(m) => m,
            Err(e) => {
                if e.downcast_ref::<Unavailable>().is_some() {
                    tracing::info!(%channel, "channel offline, checking again in {}s", OFFLINE_BACKOFF.as_secs());
                    self.with(id, |s| {
                        s.offline_until = Some(Instant::now() + OFFLINE_BACKOFF);
                        Some(())
                    });
                }
                return Err(e);
            }
        };
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
    let mut ad = primary.ad_window().map(|(from, until)| AdStatus {
        from,
        until,
        filled_by: if !s.options.slates {
            // Without slates the timeline falls behind the primary while nothing fills the ad.
            let behind = matches!((s.timeline.end(), primary.edge()), (Some(end), Some(edge)) if edge - end > 4000);
            if behind || s.options.backups.is_empty() { FilledBy::Nothing } else { FilledBy::Backup }
        } else if s.options.backups.is_empty() || s.timeline.recent_slate(3) {
            FilledBy::Slate
        } else {
            FilledBy::Backup
        },
    });
    if let Some(a) = &mut ad {
        s.break_from = Some(s.break_from.map_or(a.from, |f| f.min(a.from)));
        // Same break: keep its earliest start and latest known end. The block's first ad, which
        // carries the block's length, scrolls out of the playlist while later ones still run.
        if let Some(prev) = &s.status.ad {
            a.from = a.from.min(prev.from);
            a.until = match (a.until, prev.until) {
                (Some(x), Some(y)) => Some(x.max(y)),
                (x, y) => x.or(y),
            };
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn twitch() -> Vec<VideoRendition> {
        [(1080, 60), (720, 60), (720, 30), (480, 30), (360, 30), (160, 30)]
            .iter()
            .enumerate()
            .map(|(index, &(height, fps))| VideoRendition { index, height, fps, bandwidth: (height * fps) as u64 })
            .collect()
    }

    #[test]
    fn small_tiles_get_small_renditions() {
        let v = twitch();
        assert_eq!(choose_rendition(&v, 1080), Some(0)); // full screen: 1080p60 (the only one)
        assert_eq!(choose_rendition(&v, 720), Some(2)); // 720p30, not 720p60
        assert_eq!(choose_rendition(&v, 540), Some(3)); // a 2x2 tile: 480p is enough
        assert_eq!(choose_rendition(&v, 360), Some(4));
        assert_eq!(choose_rendition(&v, 100), Some(5));
        assert_eq!(choose_rendition(&v, 2000), Some(0));
    }

    #[test]
    fn fps_comes_from_the_name() {
        assert_eq!(fps_from_name("720p60"), 60);
        assert_eq!(fps_from_name("480p"), 30);
        assert_eq!(fps_from_name("1080p60 (source)"), 60);
    }
}
