//! Running multiview sessions (M4). A saved session (D17) starts a pipeline when a device opens
//! it, and every device that opens it shares that pipeline. Each tile reads its channel through
//! an internal single-view play session, so tiles get the same ad handling (backup chain, D20).
//! Edits to a running session are applied live. Pipelines nobody fetched for a while are stopped;
//! the saved session stays.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use nitch_media::{Config, Encoder, Multiview, TileSpec};
use serde::Serialize;

use crate::play::{FilledBy, PlayOptions, Player};
use crate::sessions::{Fit, GRID, MAX_TILES, Session, Tile};

pub const WIDTH: i32 = 1920;
pub const HEIGHT: i32 = 1080;
const FPS: i32 = 30;
const VIDEO_KBPS: u32 = 6000;
/// Stop a pipeline when no client fetched its playlists for this long.
const IDLE: Duration = Duration::from_secs(45);
/// How long `play` waits for the first segments before answering.
const READY_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Serialize)]
pub struct SlotInfo {
    pub channel: String,
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    pub z: u32,
    /// `loading` | `live` | `ad` (no ad-free source right now) | `retrying` | `offline`
    pub state: String,
}

/// What clients get from `POST`/`GET /api/sessions/{id}/play`.
#[derive(Debug, Clone, Serialize)]
pub struct MultiviewInfo {
    pub hls_url: String,
    pub grid: u32,
    /// Audio rendition `k` of the stream belongs to slot `k`.
    pub slots: Vec<Option<SlotInfo>>,
}

struct SlotTile {
    tile: Tile,
    /// Internal play session; `None` while the channel is offline.
    play_id: Option<String>,
    /// The tile's source URI (play session + rendition). Kept when the tile only moves or is
    /// resized: a new rendition would restart the source and leave the tile black for ~15 s.
    uri: Option<String>,
}

struct Running {
    mv: Multiview,
    slots: Vec<Option<SlotTile>>,
    last_access: Instant,
}

pub struct Manager {
    player: Arc<Player>,
    root: PathBuf,
    /// Base URL of this server for internal requests (tiles read our own playlists).
    base: String,
    encoder: Encoder,
    max_running: usize,
    running: Mutex<HashMap<String, Running>>,
}

/// A tile's rectangle in output pixels (60×60 grid: 32 × 18 px per cell at 1080p).
fn pixels(t: &Tile) -> (i32, i32, i32, i32) {
    let (cw, ch) = (WIDTH / GRID as i32, HEIGHT / GRID as i32);
    (t.x as i32 * cw, t.y as i32 * ch, t.w as i32 * cw, t.h as i32 * ch)
}

/// Which desired tile goes into which slot. Slots keep their channel if it's still wanted, so
/// unchanged tiles (and their audio tracks) aren't restarted.
fn assign(existing: &[Option<SlotTile>], desired: &[Tile]) -> Vec<Option<usize>> {
    let mut result = vec![None; existing.len()];
    let mut used = vec![false; desired.len()];
    for (k, slot) in existing.iter().enumerate() {
        let Some(st) = slot else { continue };
        if let Some(i) = (0..desired.len()).find(|&i| !used[i] && desired[i].channel == st.tile.channel) {
            result[k] = Some(i);
            used[i] = true;
        }
    }
    for i in (0..desired.len()).filter(|&i| !used[i]) {
        // Prefer an empty slot; otherwise take one whose tile is going away.
        let k = (0..existing.len())
            .find(|&k| result[k].is_none() && existing[k].is_none())
            .or_else(|| (0..existing.len()).find(|&k| result[k].is_none()));
        if let Some(k) = k {
            result[k] = Some(i);
        }
    }
    result
}

impl Manager {
    pub fn new(player: Arc<Player>, data_dir: &Path, port: u16, encoder: Encoder, max_running: usize) -> Result<Arc<Self>> {
        nitch_media::init()?;
        let root = data_dir.join("multiview");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root)?;
        let encoder = encoder.resolve();
        tracing::info!(?encoder, max_running, "multiview ready");
        Ok(Arc::new(Self {
            player,
            root,
            base: format!("http://127.0.0.1:{port}"),
            encoder,
            max_running,
            running: Mutex::new(HashMap::new()),
        }))
    }

    pub fn dir(&self, id: &str) -> PathBuf {
        self.root.join(id)
    }

    /// Starts the session's pipeline (or joins the running one) and waits for its first segments.
    pub async fn play(&self, session: &Session) -> Result<MultiviewInfo> {
        // A pipeline that failed is started fresh rather than joined.
        let failed = self.running.lock().unwrap().get(&session.id).and_then(|r| r.mv.failure());
        if let Some(reason) = failed {
            tracing::warn!(id = %session.id, %reason, "restarting failed multiview");
            self.stop(&session.id);
        }
        let starting = {
            let mut running = self.running.lock().unwrap();
            if !running.contains_key(&session.id) {
                if running.len() >= self.max_running {
                    bail!("{} multiviews are already running (limit {})", running.len(), self.max_running);
                }
                let mv = Multiview::start(Config {
                    dir: self.dir(&session.id),
                    width: WIDTH,
                    height: HEIGHT,
                    fps: FPS,
                    video_kbps: VIDEO_KBPS,
                    slots: MAX_TILES,
                    encoder: self.encoder.clone(),
                })?;
                tracing::info!(id = %session.id, name = %session.name, "multiview started");
                running.insert(
                    session.id.clone(),
                    Running { mv, slots: (0..MAX_TILES).map(|_| None).collect(), last_access: Instant::now() },
                );
                true
            } else {
                false
            }
        };
        if starting {
            self.apply(session).await?;
            let ready = self.dir(&session.id).join("video/index.m3u8");
            let start = Instant::now();
            while !ready.exists() && start.elapsed() < READY_TIMEOUT {
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        }
        self.info(&session.id).ok_or_else(|| anyhow::anyhow!("multiview stopped while starting"))
    }

    /// Brings a running pipeline in line with the session's tiles (no-op if it isn't running).
    pub async fn apply(&self, session: &Session) -> Result<()> {
        // Plan under the lock, start play sessions without it, then apply under it again.
        let plan: Vec<(usize, Option<Tile>, bool)> = {
            let running = self.running.lock().unwrap();
            let Some(r) = running.get(&session.id) else { return Ok(()) };
            assign(&r.slots, &session.tiles)
                .into_iter()
                .enumerate()
                .map(|(k, i)| {
                    let tile = i.map(|i| session.tiles[i].clone());
                    let same_channel = matches!((&r.slots[k], &tile), (Some(st), Some(t)) if st.tile.channel == t.channel && st.play_id.is_some());
                    (k, tile, same_channel)
                })
                .collect()
        };

        let mut new_plays = HashMap::new();
        for (k, tile, same) in &plan {
            if let (Some(t), false) = (tile, same) {
                // No slates in tiles: the slate's restarting timestamps left a tile black after
                // the break; a paused playlist is bridged by fallbacksrc instead.
                let options = PlayOptions { slates: false, ..PlayOptions::default() };
                match self.player.start(&t.channel, options).await {
                    Ok(id) => {
                        new_plays.insert(*k, id);
                    }
                    Err(e) => tracing::info!(channel = %t.channel, "multiview tile unavailable: {e:#}"),
                }
            }
        }

        let mut removed = Vec::new();
        {
            let mut running = self.running.lock().unwrap();
            let Some(r) = running.get_mut(&session.id) else { return Ok(()) };
            for (k, tile, same) in plan {
                let old = r.slots[k].take();
                let Some(tile) = tile else {
                    if let Some(old) = old {
                        removed.extend(old.play_id);
                        r.mv.set_tile(k, None)?;
                    }
                    continue;
                };
                let (play_id, kept_uri) = if same {
                    old.map_or((None, None), |o| (o.play_id, o.uri))
                } else {
                    removed.extend(old.and_then(|o| o.play_id));
                    (new_plays.remove(&k), None)
                };
                let (x, y, w, h) = pixels(&tile);
                // A new tile gets the rendition just above its size; a moved or resized one keeps
                // its source, so the picture isn't interrupted.
                let uri = kept_uri.or_else(|| {
                    let id = play_id.as_ref()?;
                    let rendition = self.player.pick_rendition(id, h as u32)?;
                    Some(format!("{}/hls/{id}/v{rendition}.m3u8", self.base))
                });
                let spec = uri.clone().map(|uri| TileSpec { uri, x, y, w, h, z: tile.z, cover: tile.fit == Fit::Cover });
                r.mv.set_tile(k, spec.as_ref())?;
                r.slots[k] = Some(SlotTile { tile, play_id, uri });
            }
        }
        for id in removed {
            self.player.remove(&id);
        }
        Ok(())
    }

    pub fn info(&self, id: &str) -> Option<MultiviewInfo> {
        let running = self.running.lock().unwrap();
        let r = running.get(id)?;
        let states = r.mv.tile_states();
        let slots = r
            .slots
            .iter()
            .zip(states)
            .map(|(slot, state)| {
                slot.as_ref().map(|st| SlotInfo {
                    channel: st.tile.channel.clone(),
                    x: st.tile.x,
                    y: st.tile.y,
                    w: st.tile.w,
                    h: st.tile.h,
                    z: st.tile.z,
                    state: {
                        let unfilled_ad = st.play_id.as_deref().and_then(|id| self.player.status(id)).and_then(|p| p.ad)
                            .is_some_and(|ad| ad.filled_by == FilledBy::Nothing);
                        match (st.play_id.is_some(), state.as_deref()) {
                            (false, _) => "offline".into(),
                            _ if unfilled_ad => "ad".into(),
                            (true, Some("running")) => "live".into(),
                            (true, Some("retrying")) => "retrying".into(),
                            _ => "loading".into(),
                        }
                    },
                })
            })
            .collect();
        Some(MultiviewInfo { hls_url: format!("/hls/mv/{id}/master.m3u8"), grid: GRID, slots })
    }

    /// Marks a session as watched (called for every playlist request).
    pub fn touch(&self, id: &str) {
        if let Some(r) = self.running.lock().unwrap().get_mut(id) {
            r.last_access = Instant::now();
        }
    }

    pub fn stop(&self, id: &str) {
        let r = self.running.lock().unwrap().remove(id);
        if let Some(r) = r {
            for st in r.slots.into_iter().flatten() {
                if let Some(play) = st.play_id {
                    self.player.remove(&play);
                }
            }
            r.mv.stop();
            tracing::info!(%id, "multiview stopped");
        }
    }

    /// Stops idle pipelines and restarts failed ones; run periodically.
    pub fn reap(&self) {
        let idle: Vec<String> = self
            .running
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, r)| r.last_access.elapsed() > IDLE || r.mv.failure().is_some())
            .map(|(id, _)| id.clone())
            .collect();
        for id in idle {
            self.stop(&id);
        }
    }

    /// Ids of running sessions whose offline tiles may have come online (retried periodically).
    pub fn with_offline_tiles(&self) -> Vec<String> {
        self.running
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, r)| r.slots.iter().flatten().any(|s| s.play_id.is_none()))
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Forgets offline tiles so the next `apply` tries them again.
    pub fn reset_offline(&self, id: &str) {
        if let Some(r) = self.running.lock().unwrap().get_mut(id) {
            for slot in &mut r.slots {
                if slot.as_ref().is_some_and(|s| s.play_id.is_none()) {
                    *slot = None;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tile(channel: &str) -> Tile {
        Tile { channel: channel.into(), x: 0, y: 0, w: 30, h: 30, z: 0, fit: Fit::Contain }
    }

    fn slot(channel: &str) -> Option<SlotTile> {
        Some(SlotTile { tile: tile(channel), play_id: Some("p".into()), uri: None })
    }

    #[test]
    fn unchanged_channels_keep_their_slot() {
        let existing = vec![slot("a"), slot("b"), slot("c"), None];
        // "b" removed, "d" added, order shuffled.
        let desired = vec![tile("c"), tile("d"), tile("a")];
        let a = assign(&existing, &desired);
        assert_eq!(a[0], Some(2), "a stays in slot 0");
        assert_eq!(a[2], Some(0), "c stays in slot 2");
        assert_eq!(a[3], Some(1), "d goes into the empty slot, not b's");
        assert_eq!(a[1], None, "b's slot is cleared");
    }

    #[test]
    fn pixels_follow_the_grid() {
        let t = Tile { channel: "x".into(), x: 30, y: 30, w: 30, h: 30, z: 0, fit: Fit::Contain };
        assert_eq!(pixels(&t), (960, 540, 960, 540));
    }
}
