//! Turns successive upstream playlists into our own live playlist with its own sequence numbers,
//! so segments can be dropped without breaking the numbering the player relies on.

use std::collections::VecDeque;
use std::fmt::Write;

use url::form_urlencoded;

use crate::hls::{MediaPlaylist, Segment};

/// Segments in the output playlist (~30 s at 2 s segments).
const WINDOW: usize = 15;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Mode {
    /// Keep everything (Twitch's own ad segments included).
    Passthrough,
    /// Drop real ad segments.
    Drop,
    /// Drop real ads, plus a fake 30 s "ad" every 2 minutes (blocks of 15 upstream segments).
    Sim,
}

impl Mode {
    pub fn parse(s: &str) -> Self {
        match s {
            "passthrough" => Self::Passthrough,
            "sim" => Self::Sim,
            _ => Self::Drop,
        }
    }

    fn drops(self, s: &Segment) -> bool {
        match self {
            Self::Passthrough => false,
            Self::Drop => s.is_ad(),
            Self::Sim => s.is_ad() || (s.seq / 15) % 4 == 3,
        }
    }
}

struct Out {
    seq: u64,
    duration: f64,
    title: String,
    pdt: Option<String>,
    uri: String,
    /// `#EXT-X-DISCONTINUITY` before this segment.
    disc: bool,
}

#[derive(Default)]
pub struct Rewriter {
    last_upstream: Option<u64>,
    next_out: u64,
    kept: VecDeque<Out>,
    pending_disc: bool,
    disc_seq: u64,
    target: f64,
    pub dropped: u64,
    pub kept_total: u64,
    pub ad_now: bool,
}

impl Rewriter {
    /// Takes the new segments of an upstream refresh. Returns human-readable events.
    pub fn update(&mut self, pl: &MediaPlaylist, mode: Mode) -> Vec<String> {
        self.target = pl.target_duration;
        let mut events = Vec::new();
        if let (Some(l), Some(last)) = (self.last_upstream, pl.segments.last()) {
            if last.seq + 10 < l {
                events.push(format!("upstream sequence reset {l} -> {}", last.seq));
                self.last_upstream = None;
                self.pending_disc = true;
            }
        }
        for s in pl.segments.iter().filter(|s| self.last_upstream.is_none_or(|l| s.seq > l)) {
            if mode.drops(s) {
                if !self.ad_now {
                    events.push(format!("ad start: dropping from upstream seq {}", s.seq));
                }
                self.ad_now = true;
                self.dropped += 1;
                self.pending_disc = true;
                continue;
            }
            if self.ad_now {
                events.push(format!("ad end: resuming at upstream seq {}", s.seq));
                self.ad_now = false;
            }
            self.kept.push_back(Out {
                seq: self.next_out,
                duration: s.duration,
                title: s.title.clone(),
                pdt: s.pdt.clone(),
                uri: s.uri.clone(),
                disc: (self.pending_disc || s.discontinuity) && self.kept_total > 0,
            });
            self.next_out += 1;
            self.kept_total += 1;
            self.pending_disc = false;
        }
        if let Some(last) = pl.segments.last() {
            self.last_upstream = Some(self.last_upstream.map_or(last.seq, |l| l.max(last.seq)));
        }
        while self.kept.len() > WINDOW {
            // A discontinuity leaving the window moves into EXT-X-DISCONTINUITY-SEQUENCE.
            if self.kept.pop_front().is_some_and(|o| o.disc) {
                self.disc_seq += 1;
            }
        }
        events
    }

    pub fn render(&self, proxy_segments: bool) -> String {
        let mut out = String::new();
        let first = self.kept.front().map_or(self.next_out, |o| o.seq);
        writeln!(out, "#EXTM3U\n#EXT-X-VERSION:3").unwrap();
        writeln!(out, "#EXT-X-TARGETDURATION:{}", self.target.ceil() as u64).unwrap();
        writeln!(out, "#EXT-X-MEDIA-SEQUENCE:{first}").unwrap();
        writeln!(out, "#EXT-X-DISCONTINUITY-SEQUENCE:{}", self.disc_seq).unwrap();
        for o in &self.kept {
            if o.disc {
                writeln!(out, "#EXT-X-DISCONTINUITY").unwrap();
            }
            if let Some(p) = &o.pdt {
                writeln!(out, "#EXT-X-PROGRAM-DATE-TIME:{p}").unwrap();
            }
            writeln!(out, "#EXTINF:{:.3},{}", o.duration, o.title).unwrap();
            if proxy_segments {
                let u: String = form_urlencoded::byte_serialize(o.uri.as_bytes()).collect();
                writeln!(out, "/seg?u={u}").unwrap();
            } else {
                writeln!(out, "{}", o.uri).unwrap();
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hls::parse_media;

    fn playlist(first: u64, titles: &[&str]) -> MediaPlaylist {
        let mut t = format!("#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:{first}\n");
        for (i, title) in titles.iter().enumerate() {
            t += &format!("#EXTINF:2.000,{title}\nseg{}.ts\n", first + i as u64);
        }
        parse_media(&t).unwrap()
    }

    #[test]
    fn dropping_keeps_numbering_contiguous_and_marks_discontinuity() {
        let mut rw = Rewriter::default();
        rw.update(&playlist(100, &["live", "live"]), Mode::Drop);
        rw.update(&playlist(101, &["live", "Amazon|1", "Amazon|2"]), Mode::Drop);
        assert!(rw.ad_now);
        rw.update(&playlist(103, &["Amazon|2", "live"]), Mode::Drop);
        assert!(!rw.ad_now);
        let out = rw.render(false);
        assert!(out.contains("#EXT-X-MEDIA-SEQUENCE:0"));
        assert_eq!(out.matches("#EXTINF").count(), 3);
        assert!(out.contains("#EXT-X-DISCONTINUITY\n#EXTINF:2.000,live\nseg104.ts"));
        assert_eq!(rw.dropped, 2);
    }
}
