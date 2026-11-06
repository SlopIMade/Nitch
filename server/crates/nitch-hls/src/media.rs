//! Media playlist parsing with ad detection (rules from spike S1, in order of reliability):
//! 1. `EXT-X-DATERANGE` with `CLASS="twitch-stitched-ad"`. It arrives ~4 s before the ad and gives
//!    the ad's start and duration.
//! 2. Segments whose title isn't `live` (ads are titled `Amazon|<creative id>`).
//!
//! `twitch-maf-ad` (display ads next to the player) doesn't touch the video and is ignored.

use anyhow::{Context, Result, bail};
use url::Url;

use crate::time::parse_pdt;
use crate::{Attrs, attr, parse_attrs};

const AD_CLASS: &str = "twitch-stitched-ad";
/// Tolerance when matching segment times against ad ranges.
const TOLERANCE_MS: i64 = 50;

#[derive(Debug, Clone)]
pub struct Segment {
    pub seq: u64,
    pub duration: f64,
    pub title: String,
    /// Absolute URI.
    pub uri: String,
    /// `PROGRAM-DATE-TIME` in Unix ms (inferred from the previous segment if missing).
    pub pdt: Option<i64>,
    /// An `EXT-X-DISCONTINUITY` precedes this segment.
    pub discontinuity: bool,
    /// The `EXT-X-MAP` init segment in effect (fMP4 streams), absolute.
    pub map: Option<String>,
    pub ad: bool,
}

impl Segment {
    pub fn end(&self) -> Option<i64> {
        self.pdt.map(|p| p + (self.duration * 1000.0).round() as i64)
    }
}

#[derive(Debug, Clone)]
pub struct DateRange {
    pub id: String,
    pub class: Option<String>,
    pub start: Option<i64>,
    pub duration: Option<f64>,
    pub attrs: Attrs,
}

impl DateRange {
    pub fn is_ad(&self) -> bool {
        self.class.as_deref() == Some(AD_CLASS) || self.id.starts_with("stitched-ad")
    }

    pub fn end(&self) -> Option<i64> {
        Some(self.start? + (self.duration? * 1000.0).round() as i64)
    }

    /// End of the whole ad block (pod), from its first ad: Twitch announces the ads of a block one
    /// by one, but the first carries the block's length (`X-TV-TWITCH-AD-POD-FILLED-DURATION`).
    /// Without it, the end jumps later with every newly announced ad.
    pub fn pod_end(&self) -> Option<i64> {
        if attr(&self.attrs, "X-TV-TWITCH-AD-POD-POSITION")? != "0" {
            return None;
        }
        let filled: f64 = attr(&self.attrs, "X-TV-TWITCH-AD-POD-FILLED-DURATION")?.parse().ok()?;
        Some(self.start? + (filled * 1000.0).round() as i64)
    }
}

#[derive(Debug, Clone)]
pub struct MediaPlaylist {
    pub target_duration: u64,
    pub segments: Vec<Segment>,
    pub dateranges: Vec<DateRange>,
    /// A backup's playlist of another rendition (lower quality) standing in for this one, e.g.
    /// the ad-free 360p source during a preroll (D36). Set by the caller.
    pub substitute: bool,
}

impl MediaPlaylist {
    /// Time ranges `[start, end)` announced as ads.
    pub fn ad_ranges(&self) -> Vec<(i64, i64)> {
        self.dateranges
            .iter()
            .filter(|d| d.is_ad())
            .filter_map(|d| {
                let end = d.end()?;
                Some((d.start?, d.pod_end().map_or(end, |pod| pod.max(end))))
            })
            .collect()
    }

    /// True if the playlist contains or announces an ad.
    pub fn has_ads(&self) -> bool {
        self.segments.iter().any(|s| s.ad) || self.dateranges.iter().any(DateRange::is_ad)
    }

    /// End of the current ad break (latest end of the announced ads), if one is in the playlist.
    pub fn ad_break_until(&self) -> Option<i64> {
        self.ad_ranges().into_iter().map(|(_, end)| end).max()
    }

    /// The ad break in this playlist in stream time: `(from, until)`. `until` is `None` while the
    /// ad still runs at the live edge without an announced end (e.g. simulated ads).
    pub fn ad_window(&self) -> Option<(i64, Option<i64>)> {
        let ranges = self.ad_ranges();
        let ads: Vec<&Segment> = self.segments.iter().filter(|s| s.ad).collect();
        let from = ranges.iter().map(|r| r.0).chain(ads.iter().filter_map(|s| s.pdt)).min()?;
        let announced = ranges.iter().map(|r| r.1).max();
        let ongoing = self.segments.last().is_some_and(|s| s.ad);
        let seen_end = (!ongoing).then(|| ads.iter().filter_map(|s| s.end()).max()).flatten();
        let until = match (announced, seen_end) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        };
        Some((from, until))
    }

    /// End time of the newest segment.
    pub fn edge(&self) -> Option<i64> {
        self.segments.last()?.end()
    }

    /// Debugging aid: treat live segments in the first `ad_s` seconds of every `period_s` seconds
    /// (by wall clock, so every rendition agrees) as ads.
    pub fn simulate_ads(&mut self, period_s: i64, ad_s: i64) {
        for s in &mut self.segments {
            if let Some(pdt) = s.pdt {
                if (pdt / 1000).rem_euclid(period_s) < ad_s {
                    s.ad = true;
                }
            }
        }
    }
}

pub fn parse_media(text: &str, base: &Url) -> Result<MediaPlaylist> {
    if !text.trim_start().starts_with("#EXTM3U") {
        bail!("not an m3u8 playlist: {}", text.chars().take(80).collect::<String>());
    }
    let mut target_duration = 2;
    let mut media_sequence = 0;
    let mut segments: Vec<Segment> = Vec::new();
    let mut dateranges = Vec::new();
    let mut pending: Option<(f64, String)> = None;
    let mut pdt: Option<i64> = None;
    let mut discontinuity = false;
    let mut map: Option<String> = None;

    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if let Some(tag) = line.strip_prefix('#') {
            let (name, value) = tag.split_once(':').unwrap_or((tag, ""));
            match name {
                "EXT-X-TARGETDURATION" => target_duration = value.parse().unwrap_or(2),
                "EXT-X-MEDIA-SEQUENCE" => media_sequence = value.parse().context("bad EXT-X-MEDIA-SEQUENCE")?,
                "EXT-X-PROGRAM-DATE-TIME" => pdt = parse_pdt(value),
                "EXT-X-DISCONTINUITY" => discontinuity = true,
                "EXT-X-MAP" => {
                    let a = parse_attrs(value);
                    map = attr(&a, "URI").and_then(|u| base.join(u).ok()).map(|u| u.to_string());
                }
                "EXTINF" => {
                    let (d, t) = value.split_once(',').unwrap_or((value, ""));
                    pending = Some((d.parse().unwrap_or(0.0), t.to_string()));
                }
                "EXT-X-DATERANGE" => {
                    let attrs = parse_attrs(value);
                    dateranges.push(DateRange {
                        id: attr(&attrs, "ID").unwrap_or_default().to_string(),
                        class: attr(&attrs, "CLASS").map(str::to_string),
                        start: attr(&attrs, "START-DATE").and_then(parse_pdt),
                        duration: attr(&attrs, "DURATION").and_then(|d| d.parse().ok()),
                        attrs,
                    });
                }
                _ => {}
            }
        } else if let Some((duration, title)) = pending.take() {
            let inferred = segments.last().and_then(Segment::end);
            segments.push(Segment {
                seq: media_sequence + segments.len() as u64,
                duration,
                ad: !title.is_empty() && title != "live",
                title,
                uri: base.join(line).map(|u| u.to_string()).unwrap_or_else(|_| line.to_string()),
                pdt: pdt.take().or(inferred),
                discontinuity: std::mem::take(&mut discontinuity),
                map: map.clone(),
            });
        }
    }

    let mut pl = MediaPlaylist { target_duration, segments, dateranges, substitute: false };
    let ranges = pl.ad_ranges();
    for s in &mut pl.segments {
        if let Some(p) = s.pdt {
            if ranges.iter().any(|(a, b)| p + TOLERANCE_MS >= *a && p < *b - TOLERANCE_MS) {
                s.ad = true;
            }
        }
    }
    Ok(pl)
}
