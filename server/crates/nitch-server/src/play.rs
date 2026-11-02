//! Single view (D1/D16): the server rewrites Twitch's playlists, clients load the segments
//! straight from the CDN. Each `POST /api/play` creates a short-lived play session.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use nitch_hls::Master;
use nitch_twitch::Twitch;
use nitch_twitch::playback::PRIMARY_PLAYER_TYPE;

/// Play sessions nobody fetched for this long are dropped.
const IDLE: Duration = Duration::from_secs(10 * 60);

struct PlaySession {
    channel: String,
    master: Master,
    last_access: Instant,
}

pub struct Player {
    tw: Twitch,
    sessions: Mutex<HashMap<String, PlaySession>>,
    counter: AtomicU64,
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
    pub async fn start(&self, channel: &str) -> Result<String> {
        let master = self.tw.master_playlist(channel, PRIMARY_PLAYER_TYPE).await?;
        let id = self.new_id();
        tracing::info!(%channel, %id, renditions = master.variants.len(), "play session started");
        let mut sessions = self.sessions.lock().unwrap();
        sessions.retain(|_, s| s.last_access.elapsed() < IDLE);
        sessions.insert(id.clone(), PlaySession { channel: channel.to_lowercase(), master, last_access: Instant::now() });
        Ok(id)
    }

    /// `None` if the session doesn't exist (anymore).
    pub fn master(&self, id: &str) -> Option<String> {
        let mut sessions = self.sessions.lock().unwrap();
        let s = sessions.get_mut(id)?;
        s.last_access = Instant::now();
        Some(nitch_hls::rewrite_master(&s.master, |i| format!("v{i}.m3u8")))
    }

    /// Media playlist of rendition `index`. Re-resolves the channel once if Twitch's URL expired.
    pub async fn media(&self, id: &str, index: usize) -> Result<Option<String>> {
        let Some((channel, url)) = self.variant(id, index) else { return Ok(None) };
        let (status, text) = self.tw.get_text(&url).await?;
        if status.is_success() {
            nitch_hls::check_media(&text)?;
            return Ok(Some(text));
        }
        tracing::info!(%channel, %status, "media playlist rejected, resolving the channel again");
        let master = self.tw.master_playlist(&channel, PRIMARY_PLAYER_TYPE).await?;
        let url = master.variants.get(index).context("rendition disappeared")?.uri.clone();
        if let Some(s) = self.sessions.lock().unwrap().get_mut(id) {
            s.master = master;
        }
        let (status, text) = self.tw.get_text(&url).await?;
        anyhow::ensure!(status.is_success(), "media playlist returned {status}");
        nitch_hls::check_media(&text)?;
        Ok(Some(text))
    }

    fn variant(&self, id: &str, index: usize) -> Option<(String, url::Url)> {
        let mut sessions = self.sessions.lock().unwrap();
        let s = sessions.get_mut(id)?;
        s.last_access = Instant::now();
        Some((s.channel.clone(), s.master.variants.get(index)?.uri.clone()))
    }
}
