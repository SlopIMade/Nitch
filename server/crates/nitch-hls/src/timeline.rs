//! The output timeline of a single-view play session (M3, D15).
//!
//! Every live segment is identified by its `PROGRAM-DATE-TIME`, which is the same in every
//! rendition and in the backup player type (S2). The timeline is shared by all renditions of a
//! session and numbers entries in time order, so quality switches stay aligned. For each stretch
//! of time it takes, in this order:
//! 1. the primary playlist's live segment,
//! 2. during an ad, the first backup (in priority order) that has the live segment at that time.
//!    It's byte-identical, so the swap is seamless and needs no discontinuity. Every backup is
//!    its own Twitch session with its own ad schedule, so more backups mean fewer slates,
//! 3. only if every source is in an ad: a slate segment of the usual length, once it's certain
//!    no live segment will come.
//!
//! Ad segments never reach the output, and the timeline never has a hole (S2).

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fmt::Write;

use crate::media::MediaPlaylist;
use crate::time::format_pdt;

/// Segments in the output playlist.
pub const WINDOW: usize = 15;
/// Seconds behind the live edge a multiview tile starts (playlists without slates).
const TILE_HOLD_BACK: f64 = 4.0;
/// Entries kept in memory (older ones are pruned).
const KEEP: usize = 90;
/// Two segment times closer than this are the same segment.
const TOLERANCE_MS: i64 = 150;
/// A stretch without live content waits this long for the backups before it becomes a slate.
const GRACE_MS: i64 = 4000;
/// How far behind the wall clock Twitch's live edge usually is. During an ad, with a wall clock,
/// slates advance with real time up to here (minus the grace), instead of with the ad segments'
/// timestamps. Too little only means slates run a bit ahead and the first live seconds after the
/// break are skipped.
const LIVE_LAG_MS: i64 = 6000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Origin {
    Primary,
    /// Index into the backup list passed to [`Timeline::update`].
    Backup(usize),
    Slate,
}

#[derive(Debug, Clone)]
struct Entry {
    seq: u64,
    start: i64,
    duration: f64,
    origin: Origin,
    /// From a substitute rendition (lower quality, see [`MediaPlaylist::substitute`]).
    substitute: bool,
    /// `EXT-X-DISCONTINUITY` before this entry.
    disc: bool,
}

#[derive(Debug, Clone)]
struct Source {
    uri: String,
    map: Option<String>,
}

/// Where slate segments come from; fMP4 streams need an fMP4 slate with its own init segment,
/// because an `EXT-X-MAP` applies to every following segment.
pub struct Slates<'a> {
    pub ts: &'a str,
    pub fmp4_init: &'a str,
    pub fmp4: &'a str,
}

struct Candidate {
    duration: f64,
    origin: Origin,
    /// A real discontinuity in the live stream (e.g. a stream restart), not one around an ad.
    genuine_disc: bool,
    substitute: bool,
}

pub struct Timeline {
    entries: VecDeque<Entry>,
    next_seq: u64,
    /// Discontinuities that were pruned from the front.
    pruned_disc: u64,
    /// Usual live segment duration (Twitch uses 2.000 s or e.g. 1.985 s).
    nominal: f64,
    /// Per rendition: segment time → URI.
    sources: HashMap<usize, BTreeMap<i64, Source>>,
    /// Renditions known to be fMP4 (from any primary segment, ads included): their slates must be
    /// fMP4 from the very first one. Deciding by live segments only, a preroll's slates went out
    /// as TS until the stream began, and the switch broke the player (bufferAppendError).
    fmp4: std::collections::HashSet<usize>,
    /// Fill unfillable ad time with slates (browser player, D15). Without slates the playlist
    /// pauses and continues with the next live segment (multiview tiles: fallbacksrc retimes
    /// a stalled input, and slate timestamps confused the tile's timeline).
    slates: bool,
}

fn ms(seconds: f64) -> i64 {
    (seconds * 1000.0).round() as i64
}

impl Default for Timeline {
    fn default() -> Self {
        Self::new()
    }
}

impl Timeline {
    pub fn new() -> Self {
        Self::with_slates(true)
    }

    pub fn with_slates(slates: bool) -> Self {
        Self {
            entries: VecDeque::new(),
            next_seq: 0,
            pruned_disc: 0,
            nominal: 2.0,
            sources: HashMap::new(),
            fmp4: Default::default(),
            slates,
        }
    }

    /// Takes a fresh primary playlist of `rendition` (and, during ads, the backups' in priority
    /// order; `None` where a backup isn't available) and extends the timeline as far as possible.
    pub fn update(&mut self, rendition: usize, primary: &MediaPlaylist, backups: &[Option<&MediaPlaylist>]) {
        self.update_at(rendition, primary, backups, None);
    }

    /// [`Timeline::update`], with the wall clock (Unix ms) for slates: they keep pace with real
    /// time even when the ad segments' timestamps don't (a preroll's stood still for half a
    /// minute, so the output stalled and later jumped ahead).
    pub fn update_at(&mut self, rendition: usize, primary: &MediaPlaylist, backups: &[Option<&MediaPlaylist>], now: Option<i64>) {
        if primary.segments.iter().any(|s| s.map.is_some()) {
            self.fmp4.insert(rendition);
        }
        let sources = self.sources.entry(rendition).or_default();
        let mut live: BTreeMap<i64, Candidate> = BTreeMap::new();

        for (i, s) in primary.segments.iter().enumerate() {
            let Some(pdt) = s.pdt else { continue };
            if s.ad {
                continue;
            }
            let prev_live = i > 0 && !primary.segments[i - 1].ad;
            sources.insert(pdt, Source { uri: s.uri.clone(), map: s.map.clone() });
            live.insert(pdt, Candidate { duration: s.duration, origin: Origin::Primary, genuine_disc: s.discontinuity && prev_live, substitute: false });
            if s.duration > 0.5 {
                self.nominal = s.duration;
            }
        }
        // Earlier backups win: a time already covered (by the primary or a better backup) is skipped.
        for (index, backup) in backups.iter().enumerate() {
            let Some(backup) = backup else { continue };
            for s in backup.segments.iter().filter(|s| !s.ad) {
                let Some(pdt) = s.pdt else { continue };
                if live.range(pdt - TOLERANCE_MS..=pdt + TOLERANCE_MS).next().is_some() {
                    continue;
                }
                sources.entry(pdt).or_insert(Source { uri: s.uri.clone(), map: s.map.clone() });
                live.insert(pdt, Candidate { duration: s.duration, origin: Origin::Backup(index), genuine_disc: false, substitute: backup.substitute });
            }
        }

        let (Some(start), Some(edge)) = (primary.segments.first().and_then(|s| s.pdt), primary.edge()) else {
            return;
        };
        // Only while the primary is in an ad: otherwise its own edge is the truth, however far
        // behind the wall clock it is.
        let in_ad = primary.segments.last().is_some_and(|s| s.ad);
        let edge = match now {
            Some(now) if in_ad => edge.max(now - LIVE_LAG_MS),
            _ => edge,
        };
        self.advance(&live, start, edge);
        self.prune();
    }

    fn advance(&mut self, live: &BTreeMap<i64, Candidate>, start: i64, edge: i64) {
        let mut end = self.entries.back().map_or(start, |e| e.start + ms(e.duration));
        loop {
            // Back to the primary as soon as it has live content here again. A backup (the 360p
            // substitute especially) can run on a time grid half a second off; following it,
            // every primary segment started "too early" and was skipped until the backups were
            // no longer fetched, ~30 s after the ad. The discontinuity covers the small jump.
            if self.last_origin().is_some_and(|o| matches!(o, Origin::Backup(_))) {
                let back = live
                    .range(end - ms(self.nominal) / 2..end + ms(self.nominal))
                    .find(|(_, c)| c.origin == Origin::Primary)
                    .map(|(&pdt, c)| (pdt, c.duration));
                if let Some((pdt, duration)) = back {
                    // Same rule as taking a segment normally: within half a segment is drift.
                    let off_grid = (pdt - end).abs() >= ms(self.nominal) / 2;
                    self.push(pdt, duration, Origin::Primary, off_grid, false);
                    end = pdt + ms(duration);
                    continue;
                }
            }
            let next = live.range(end - TOLERANCE_MS..).next();
            match next {
                // The next live segment starts here (within half a segment, which absorbs
                // rounding and drift like 1.985 s segments): take it.
                Some((&pdt, c)) if pdt - end < ms(self.nominal) / 2 => {
                    self.push_live(pdt, c);
                    end = pdt + ms(c.duration);
                }
                // Without slates: skip the gap and continue with the next live segment.
                Some((&pdt, c)) if !self.slates => {
                    self.push_live(pdt, c);
                    end = pdt + ms(c.duration);
                }
                // After a slate (or at the start), live content starts less than a slate from
                // here: ads run on their own time grid, live segments on another, so no slate fits
                // in between. Take it; the discontinuity after a slate covers the short gap.
                // Waiting for a slate that can never fit froze the output until the preroll ended.
                Some((&pdt, c)) if pdt - end < ms(self.nominal) && self.last_origin().is_none_or(|o| o == Origin::Slate) => {
                    self.push_live(pdt, c);
                    end = pdt + ms(c.duration);
                }
                None if !self.slates => break,
                // No live content at `end`. A slate fills it once that's certain: either live
                // content resumes later, or the primary has moved on long enough for the backup.
                _ => {
                    let slot_end = end + ms(self.nominal);
                    let limit = next.map_or(edge - GRACE_MS, |(&pdt, _)| pdt + TOLERANCE_MS);
                    if slot_end > limit {
                        break;
                    }
                    self.push(end, self.nominal, Origin::Slate, false, false);
                    end = slot_end;
                }
            }
        }
    }

    fn push_live(&mut self, start: i64, c: &Candidate) {
        self.push(start, c.duration, c.origin, c.genuine_disc, c.substitute);
    }

    fn push(&mut self, start: i64, duration: f64, origin: Origin, genuine_disc: bool, substitute: bool) {
        let prev = self.entries.back();
        let prev_slate = prev.is_some_and(|e| e.origin == Origin::Slate);
        // A switch to or from a substitute rendition changes resolution and init segment.
        let rendition_change = prev.is_some_and(|e| e.origin != Origin::Slate && e.substitute != substitute);
        // Each slate segment restarts its timestamps, so it needs a discontinuity, and so does
        // the first live segment after one. Primary and backup are the same transcode: no need.
        let disc = !self.entries.is_empty() && (origin == Origin::Slate || prev_slate || genuine_disc || rendition_change);
        self.entries.push_back(Entry { seq: self.next_seq, start, duration, origin, substitute, disc });
        self.next_seq += 1;
    }

    fn prune(&mut self) {
        while self.entries.len() > KEEP {
            if self.entries.pop_front().is_some_and(|e| e.disc) {
                self.pruned_disc += 1;
            }
        }
        if let Some(first) = self.entries.front().map(|e| e.start) {
            for s in self.sources.values_mut() {
                *s = s.split_off(&(first - TOLERANCE_MS));
            }
        }
    }

    /// Stream time up to which the timeline is filled.
    pub fn end(&self) -> Option<i64> {
        self.entries.back().map(|e| e.start + ms(e.duration))
    }

    /// Origin of the newest entry.
    pub fn last_origin(&self) -> Option<Origin> {
        self.entries.back().map(|e| e.origin)
    }

    /// True if the newest entry comes from a substitute rendition (lower quality).
    pub fn last_substitute(&self) -> bool {
        self.entries.back().is_some_and(|e| e.substitute)
    }

    /// True if any of the newest `n` entries is a slate.
    pub fn recent_slate(&self, n: usize) -> bool {
        self.entries.iter().rev().take(n).any(|e| e.origin == Origin::Slate)
    }

    /// How many entries since stream time `from` came from each origin (for logging a break).
    pub fn origins_since(&self, from: i64) -> HashMap<Origin, usize> {
        let mut counts = HashMap::new();
        for e in self.entries.iter().filter(|e| e.start + TOLERANCE_MS >= from) {
            *counts.entry(e.origin).or_insert(0) += 1;
        }
        counts
    }

    /// The media playlist for `rendition`.
    pub fn render(&self, rendition: usize, slates: &Slates) -> String {
        let empty = BTreeMap::new();
        let sources = self.sources.get(&rendition).unwrap_or(&empty);
        let fmp4 = self.fmp4.contains(&rendition) || sources.values().any(|s| s.map.is_some());
        let from = self.entries.len().saturating_sub(WINDOW);
        let disc_seq = self.pruned_disc + self.entries.iter().take(from).filter(|e| e.disc).count() as u64;

        let mut body = String::new();
        let mut max_duration: f64 = 2.0;
        let mut last_map: Option<&str> = None;
        let mut first_seq = None;
        for e in self.entries.iter().skip(from) {
            let (uri, map, title) = match e.origin {
                Origin::Slate if fmp4 => (slates.fmp4, Some(slates.fmp4_init), "slate"),
                Origin::Slate => (slates.ts, None, "slate"),
                _ => match sources.get(&e.start) {
                    Some(s) => (s.uri.as_str(), s.map.as_deref(), "live"),
                    // This rendition hasn't seen that segment yet: stop here rather than guess.
                    None => break,
                },
            };
            first_seq.get_or_insert(e.seq);
            if e.disc {
                body.push_str("#EXT-X-DISCONTINUITY\n");
                last_map = None;
            }
            if let Some(m) = map {
                if last_map != Some(m) {
                    writeln!(body, "#EXT-X-MAP:URI=\"{m}\"").unwrap();
                    last_map = Some(m);
                }
            }
            writeln!(body, "#EXT-X-PROGRAM-DATE-TIME:{}", format_pdt(e.start)).unwrap();
            writeln!(body, "#EXTINF:{:.3},{title}\n{uri}", e.duration).unwrap();
            max_duration = max_duration.max(e.duration);
        }

        let mut out = String::new();
        writeln!(out, "#EXTM3U\n#EXT-X-VERSION:6").unwrap();
        writeln!(out, "#EXT-X-TARGETDURATION:{}", max_duration.ceil() as u64).unwrap();
        writeln!(out, "#EXT-X-MEDIA-SEQUENCE:{}", first_seq.unwrap_or(self.next_seq)).unwrap();
        writeln!(out, "#EXT-X-DISCONTINUITY-SEQUENCE:{disc_seq}").unwrap();
        if !self.slates {
            // Multiview tiles (GStreamer's hlsdemux2): start two segments behind the live edge
            // instead of the default three target durations, which shortens the delay.
            writeln!(out, "#EXT-X-SERVER-CONTROL:HOLD-BACK={TILE_HOLD_BACK:.1}").unwrap();
        }
        out.push_str(&body);
        out
    }
}
