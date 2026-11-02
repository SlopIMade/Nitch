//! The output timeline of a single-view play session (M3, D15).
//!
//! Every live segment is identified by its `PROGRAM-DATE-TIME`, which is the same in every
//! rendition and in the backup player type (S2). The timeline is shared by all renditions of a
//! session and numbers entries in time order, so quality switches stay aligned. For each stretch
//! of time it takes, in this order:
//! 1. the primary playlist's live segment,
//! 2. during an ad, the backup playlist's live segment at the same time (byte-identical, so the swap
//!    is seamless and needs no discontinuity),
//! 3. otherwise a slate segment of the usual length, once it's certain no live segment will come.
//!
//! Ad segments never reach the output, and the timeline never has a hole (S2).

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fmt::Write;

use crate::media::MediaPlaylist;
use crate::time::format_pdt;

/// Segments in the output playlist.
pub const WINDOW: usize = 15;
/// Entries kept in memory (older ones are pruned).
const KEEP: usize = 90;
/// Two segment times closer than this are the same segment.
const TOLERANCE_MS: i64 = 150;
/// A stretch without live content waits this long for the backup before it becomes a slate.
const GRACE_MS: i64 = 4000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Primary,
    Backup,
    Slate,
}

#[derive(Debug, Clone)]
struct Entry {
    seq: u64,
    start: i64,
    duration: f64,
    origin: Origin,
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
        Self { entries: VecDeque::new(), next_seq: 0, pruned_disc: 0, nominal: 2.0, sources: HashMap::new() }
    }

    /// Takes a fresh primary playlist of `rendition` (and the backup's, during ads) and extends
    /// the timeline as far as possible.
    pub fn update(&mut self, rendition: usize, primary: &MediaPlaylist, backup: Option<&MediaPlaylist>) {
        let sources = self.sources.entry(rendition).or_default();
        let mut live: BTreeMap<i64, Candidate> = BTreeMap::new();

        for (i, s) in primary.segments.iter().enumerate() {
            let Some(pdt) = s.pdt else { continue };
            if s.ad {
                continue;
            }
            let prev_live = i > 0 && !primary.segments[i - 1].ad;
            sources.insert(pdt, Source { uri: s.uri.clone(), map: s.map.clone() });
            live.insert(pdt, Candidate { duration: s.duration, origin: Origin::Primary, genuine_disc: s.discontinuity && prev_live });
            if s.duration > 0.5 {
                self.nominal = s.duration;
            }
        }
        if let Some(backup) = backup {
            for s in backup.segments.iter().filter(|s| !s.ad) {
                let Some(pdt) = s.pdt else { continue };
                if live.range(pdt - TOLERANCE_MS..=pdt + TOLERANCE_MS).next().is_some() {
                    continue;
                }
                sources.entry(pdt).or_insert(Source { uri: s.uri.clone(), map: s.map.clone() });
                live.insert(pdt, Candidate { duration: s.duration, origin: Origin::Backup, genuine_disc: false });
            }
        }

        let (Some(start), Some(edge)) = (primary.segments.first().and_then(|s| s.pdt), primary.edge()) else {
            return;
        };
        self.advance(&live, start, edge);
        self.prune();
    }

    fn advance(&mut self, live: &BTreeMap<i64, Candidate>, start: i64, edge: i64) {
        let mut end = self.entries.back().map_or(start, |e| e.start + ms(e.duration));
        loop {
            let next = live.range(end - TOLERANCE_MS..).next();
            match next {
                // The next live segment starts here (within half a segment, which absorbs
                // rounding and drift like 1.985 s segments): take it.
                Some((&pdt, c)) if pdt - end < ms(self.nominal) / 2 => {
                    self.push(pdt, c.duration, c.origin, c.genuine_disc);
                    end = pdt + ms(c.duration);
                }
                // No live content at `end`. A slate fills it once that's certain: either live
                // content resumes later, or the primary has moved on long enough for the backup.
                _ => {
                    let slot_end = end + ms(self.nominal);
                    let limit = next.map_or(edge - GRACE_MS, |(&pdt, _)| pdt + TOLERANCE_MS);
                    if slot_end > limit {
                        break;
                    }
                    self.push(end, self.nominal, Origin::Slate, false);
                    end = slot_end;
                }
            }
        }
    }

    fn push(&mut self, start: i64, duration: f64, origin: Origin, genuine_disc: bool) {
        let prev_slate = self.entries.back().is_some_and(|e| e.origin == Origin::Slate);
        // Each slate segment restarts its timestamps, so it needs a discontinuity, and so does
        // the first live segment after one. Primary and backup are the same transcode: no need.
        let disc = !self.entries.is_empty() && (origin == Origin::Slate || prev_slate || genuine_disc);
        self.entries.push_back(Entry { seq: self.next_seq, start, duration, origin, disc });
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

    /// Origin of the newest entry.
    pub fn last_origin(&self) -> Option<Origin> {
        self.entries.back().map(|e| e.origin)
    }

    /// True if any of the newest `n` entries is a slate.
    pub fn recent_slate(&self, n: usize) -> bool {
        self.entries.iter().rev().take(n).any(|e| e.origin == Origin::Slate)
    }

    /// The media playlist for `rendition`.
    pub fn render(&self, rendition: usize, slates: &Slates) -> String {
        let empty = BTreeMap::new();
        let sources = self.sources.get(&rendition).unwrap_or(&empty);
        let fmp4 = sources.values().any(|s| s.map.is_some());
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
        out.push_str(&body);
        out
    }
}
