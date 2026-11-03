//! Read-only Twitch chat (M6, D6): one anonymous IRC-over-WebSocket connection for the whole
//! server. Channels are joined while at least one client wants them (reference counted) and
//! parted when nobody does. Messages go to every WebSocket client through a broadcast channel;
//! each client filters for the channels it shows (focused stream, or the combined set).
//!
//! History: the last [`HISTORY`] messages of every joined channel are kept, and a client gets them
//! when it subscribes, so a chat opens with what was said before. Twitch offers no history, so
//! the server reads the followed live channels in the background ("pinned") to have some.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Mutex;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use serde::Serialize;
use tokio::sync::{broadcast, mpsc};
use tokio_tungstenite::tungstenite::Message;

use crate::sessions::valid_channel;

const IRC_URL: &str = "wss://irc-ws.chat.twitch.tv:443";
/// Messages kept per joined channel.
pub const HISTORY: usize = 150;

/// One chat line, as sent to clients.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ChatMessage {
    pub channel: String,
    pub user: String,
    pub display_name: String,
    /// `#RRGGBB`, or empty if the user never chose one.
    pub color: String,
    pub text: String,
    /// `[emote id, first char, last char]`, char positions counting Unicode code points.
    pub emotes: Vec<(String, usize, usize)>,
    /// e.g. `["broadcaster/1", "subscriber/12"]`
    pub badges: Vec<String>,
    /// Unix ms (Twitch's `tmi-sent-ts`).
    pub ts: i64,
}

/// Parses the tags of an IRC line (`@a=1;b=2 :prefix CMD ...`) into a map, unescaping values.
fn parse_tags(raw: &str) -> HashMap<&str, String> {
    raw.split(';')
        .filter_map(|kv| {
            let (k, v) = kv.split_once('=')?;
            let v = v.replace("\\s", " ").replace("\\:", ";").replace("\\\\", "\\").replace("\\r", "").replace("\\n", "");
            Some((k, v))
        })
        .collect()
}

/// Parses a `PRIVMSG` line; other lines give `None`.
pub fn parse_privmsg(line: &str) -> Option<ChatMessage> {
    let (tags, rest) = match line.strip_prefix('@') {
        Some(l) => {
            let (t, r) = l.split_once(' ')?;
            (parse_tags(t), r)
        }
        None => (HashMap::new(), line),
    };
    // :user!user@user.tmi.twitch.tv PRIVMSG #channel :text
    let rest = rest.strip_prefix(':')?;
    let (prefix, rest) = rest.split_once(' ')?;
    let rest = rest.strip_prefix("PRIVMSG #")?;
    let (channel, text) = rest.split_once(" :")?;
    let user = prefix.split('!').next()?.to_string();
    let emotes = tags
        .get("emotes")
        .map(|e| {
            e.split('/')
                .filter_map(|group| {
                    let (id, ranges) = group.split_once(':')?;
                    Some(ranges.split(',').filter_map(move |r| {
                        let (a, b) = r.split_once('-')?;
                        Some((id.to_string(), a.parse().ok()?, b.parse().ok()?))
                    }))
                })
                .flatten()
                .collect()
        })
        .unwrap_or_default();
    Some(ChatMessage {
        channel: channel.to_string(),
        display_name: tags.get("display-name").filter(|d| !d.is_empty()).cloned().unwrap_or_else(|| user.clone()),
        user,
        color: tags.get("color").cloned().unwrap_or_default(),
        text: text.trim_end_matches(['\r', '\n']).to_string(),
        emotes,
        badges: tags.get("badges").map(|b| b.split(',').filter(|s| !s.is_empty()).map(str::to_string).collect()).unwrap_or_default(),
        ts: tags.get("tmi-sent-ts").and_then(|t| t.parse().ok()).unwrap_or(0),
    })
}

enum Cmd {
    Join(String),
    Part(String),
}

pub struct ChatHub {
    cmds: mpsc::UnboundedSender<Cmd>,
    messages: broadcast::Sender<ChatMessage>,
    counts: Mutex<HashMap<String, usize>>,
    /// The latest messages of each joined channel, oldest first.
    history: Mutex<HashMap<String, VecDeque<ChatMessage>>>,
    /// Channels read in the background (each holds one subscription).
    pinned: Mutex<HashSet<String>>,
}

impl ChatHub {
    pub fn start() -> std::sync::Arc<Self> {
        let (cmds, rx) = mpsc::unbounded_channel();
        let (messages, _) = broadcast::channel(1024);
        tokio::spawn(run(rx, messages.clone()));
        let hub = std::sync::Arc::new(Self {
            cmds,
            counts: Mutex::new(HashMap::new()),
            history: Mutex::new(HashMap::new()),
            pinned: Mutex::new(HashSet::new()),
            messages,
        });
        // Record every message of a joined channel.
        let recorder = hub.clone();
        let mut rx = hub.messages.subscribe();
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(m) => recorder.record(m),
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => return,
                }
            }
        });
        hub
    }

    fn record(&self, m: ChatMessage) {
        if !self.counts.lock().unwrap().contains_key(&m.channel) {
            return;
        }
        let mut history = self.history.lock().unwrap();
        let h = history.entry(m.channel.clone()).or_default();
        h.push_back(m);
        while h.len() > HISTORY {
            h.pop_front();
        }
    }

    /// The latest messages of `channel` (empty if it isn't read).
    pub fn history(&self, channel: &str) -> Vec<ChatMessage> {
        self.history.lock().unwrap().get(&channel.to_lowercase()).map(|h| h.iter().cloned().collect()).unwrap_or_default()
    }

    /// Sets the channels read in the background (the followed live ones).
    pub fn set_pinned(&self, channels: HashSet<String>) {
        let old = std::mem::replace(&mut *self.pinned.lock().unwrap(), channels.clone());
        for c in old.difference(&channels) {
            self.unsubscribe(c);
        }
        for c in channels.difference(&old) {
            self.subscribe(c);
        }
    }

    pub fn messages(&self) -> broadcast::Receiver<ChatMessage> {
        self.messages.subscribe()
    }

    /// Joined channels and how many clients want each (diagnostics).
    pub fn channels(&self) -> HashMap<String, usize> {
        self.counts.lock().unwrap().clone()
    }

    /// A client wants `channel`'s chat; joins it if it's the first.
    pub fn subscribe(&self, channel: &str) {
        let channel = channel.to_lowercase();
        if !valid_channel(&channel) {
            return;
        }
        let mut counts = self.counts.lock().unwrap();
        let n = counts.entry(channel.clone()).or_insert(0);
        *n += 1;
        if *n == 1 {
            let _ = self.cmds.send(Cmd::Join(channel));
        }
    }

    /// A client no longer wants `channel`; parts it if it was the last.
    pub fn unsubscribe(&self, channel: &str) {
        let channel = channel.to_lowercase();
        let mut counts = self.counts.lock().unwrap();
        if let Some(n) = counts.get_mut(&channel) {
            *n -= 1;
            if *n == 0 {
                counts.remove(&channel);
                self.history.lock().unwrap().remove(&channel);
                let _ = self.cmds.send(Cmd::Part(channel));
            }
        }
    }
}

/// The connection task: connects, joins what's wanted, reconnects with backoff.
async fn run(mut cmds: mpsc::UnboundedReceiver<Cmd>, out: broadcast::Sender<ChatMessage>) {
    let mut wanted: HashSet<String> = HashSet::new();
    let mut backoff = Duration::from_secs(1);
    loop {
        // Nothing to read: wait for the first join instead of holding a connection open.
        while wanted.is_empty() {
            match cmds.recv().await {
                Some(Cmd::Join(c)) => {
                    wanted.insert(c);
                }
                Some(Cmd::Part(c)) => {
                    wanted.remove(&c);
                }
                None => return,
            }
        }
        match session(&mut cmds, &out, &mut wanted).await {
            Ok(()) => backoff = Duration::from_secs(1),
            Err(e) => {
                tracing::warn!("chat connection lost: {e:#}; reconnecting in {}s", backoff.as_secs());
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(60));
            }
        }
    }
}

/// One connection; returns `Ok` when nothing is wanted anymore, `Err` when the connection broke.
async fn session(
    cmds: &mut mpsc::UnboundedReceiver<Cmd>,
    out: &broadcast::Sender<ChatMessage>,
    wanted: &mut HashSet<String>,
) -> anyhow::Result<()> {
    let (ws, _) = tokio_tungstenite::connect_async(IRC_URL).await?;
    let (mut tx, mut rx) = ws.split();
    // Anonymous, read-only: the justinfan nick needs no password.
    for line in ["CAP REQ :twitch.tv/tags twitch.tv/commands", "PASS SCHMOOPIIE", "NICK justinfan41827"] {
        tx.send(Message::text(line)).await?;
    }
    for c in wanted.iter() {
        tx.send(Message::text(format!("JOIN #{c}"))).await?;
    }
    tracing::info!(channels = wanted.len(), "chat connected");
    loop {
        tokio::select! {
            cmd = cmds.recv() => match cmd {
                Some(Cmd::Join(c)) => {
                    if wanted.insert(c.clone()) {
                        tx.send(Message::text(format!("JOIN #{c}"))).await?;
                    }
                }
                Some(Cmd::Part(c)) => {
                    if wanted.remove(&c) {
                        tx.send(Message::text(format!("PART #{c}"))).await?;
                    }
                    if wanted.is_empty() {
                        let _ = tx.close().await;
                        tracing::info!("chat disconnected (no channels)");
                        return Ok(());
                    }
                }
                None => return Ok(()),
            },
            frame = rx.next() => {
                let Some(frame) = frame else { anyhow::bail!("closed by Twitch") };
                let Message::Text(text) = frame? else { continue };
                for line in text.lines() {
                    if let Some(rest) = line.strip_prefix("PING") {
                        tx.send(Message::text(format!("PONG{rest}"))).await?;
                    } else if let Some(msg) = parse_privmsg(line) {
                        let _ = out.send(msg);
                    } else if line.contains(" RECONNECT") {
                        anyhow::bail!("Twitch asked to reconnect");
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_privmsg_with_tags_and_emotes() {
        let line = "@badge-info=subscriber/14;badges=subscriber/12,premium/1;color=#1E90FF;display-name=Some\\sOne;\
            emotes=25:0-4,12-16/1902:6-10;id=abc;tmi-sent-ts=1793660000000 \
            :someone!someone@someone.tmi.twitch.tv PRIVMSG #tarik :Kappa Keepo Kappa";
        let m = parse_privmsg(line).unwrap();
        assert_eq!(m.channel, "tarik");
        assert_eq!(m.user, "someone");
        assert_eq!(m.display_name, "Some One");
        assert_eq!(m.color, "#1E90FF");
        assert_eq!(m.text, "Kappa Keepo Kappa");
        assert_eq!(m.badges, vec!["subscriber/12", "premium/1"]);
        assert_eq!(m.ts, 1793660000000);
        assert!(m.emotes.contains(&("25".into(), 0, 4)));
        assert!(m.emotes.contains(&("25".into(), 12, 16)));
        assert!(m.emotes.contains(&("1902".into(), 6, 10)));
    }

    #[test]
    fn ignores_other_lines() {
        assert!(parse_privmsg(":tmi.twitch.tv 001 justinfan41827 :Welcome, GLHF!").is_none());
        assert!(parse_privmsg("PING :tmi.twitch.tv").is_none());
        assert!(parse_privmsg("@emote-only=0 :tmi.twitch.tv ROOMSTATE #tarik").is_none());
    }

    #[test]
    fn falls_back_to_login_without_display_name() {
        let m = parse_privmsg(":a!a@a.tmi.twitch.tv PRIVMSG #b :hi\r").unwrap();
        assert_eq!((m.display_name.as_str(), m.text.as_str(), m.emotes.len()), ("a", "hi", 0));
    }
}
