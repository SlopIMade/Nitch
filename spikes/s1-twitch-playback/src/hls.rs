//! Minimal line-based HLS parsing. Keeps every tag name so unknown Twitch tags show up.

use std::collections::{BTreeSet, HashMap};

use anyhow::{Context, Result, bail};
use url::Url;

pub type Attrs = Vec<(String, String)>;

/// Splits an attribute list like `A=1,B="x,y"` into key/value pairs, without quotes.
pub fn parse_attrs(s: &str) -> Attrs {
    let mut out = Vec::new();
    let mut rest = s.trim();
    while let Some(eq) = rest.find('=') {
        let key = rest[..eq].trim().to_string();
        rest = &rest[eq + 1..];
        let value;
        if let Some(quoted) = rest.strip_prefix('"') {
            let end = quoted.find('"').unwrap_or(quoted.len());
            value = quoted[..end].to_string();
            rest = quoted.get(end + 1..).unwrap_or("");
        } else {
            let end = rest.find(',').unwrap_or(rest.len());
            value = rest[..end].to_string();
            rest = &rest[end..];
        }
        rest = rest.strip_prefix(',').unwrap_or(rest).trim_start();
        out.push((key, value));
    }
    out
}

pub fn attr<'a>(attrs: &'a Attrs, key: &str) -> Option<&'a str> {
    attrs.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

pub struct Variant {
    pub name: String,
    pub resolution: Option<String>,
    pub bandwidth: u64,
    pub codecs: Option<String>,
    pub uri: Url,
}

pub struct Master {
    pub variants: Vec<Variant>,
}

pub fn parse_master(text: &str, base: &Url) -> Result<Master> {
    let mut names = HashMap::new(); // GROUP-ID -> NAME
    let mut pending: Option<Attrs> = None;
    let mut variants = Vec::new();
    for line in text.lines().map(str::trim) {
        if let Some(a) = line.strip_prefix("#EXT-X-MEDIA:") {
            let a = parse_attrs(a);
            if let (Some(g), Some(n)) = (attr(&a, "GROUP-ID"), attr(&a, "NAME")) {
                names.insert(g.to_string(), n.to_string());
            }
        } else if let Some(a) = line.strip_prefix("#EXT-X-STREAM-INF:") {
            pending = Some(parse_attrs(a));
        } else if !line.is_empty() && !line.starts_with('#') {
            if let Some(a) = pending.take() {
                let group = attr(&a, "VIDEO").unwrap_or("");
                variants.push(Variant {
                    name: names.get(group).cloned().unwrap_or_else(|| group.to_string()),
                    resolution: attr(&a, "RESOLUTION").map(str::to_string),
                    bandwidth: attr(&a, "BANDWIDTH").and_then(|b| b.parse().ok()).unwrap_or(0),
                    codecs: attr(&a, "CODECS").map(str::to_string),
                    uri: base.join(line)?,
                });
            }
        }
    }
    if variants.is_empty() {
        bail!("master playlist has no variants");
    }
    Ok(Master { variants })
}

pub struct Segment {
    pub seq: u64,
    pub duration: f64,
    pub title: String,
}

impl Segment {
    /// Live content segments are titled "live"; stitched ads carry other titles.
    pub fn is_ad(&self) -> bool {
        !self.title.is_empty() && self.title != "live"
    }
}

pub struct DateRange {
    pub id: String,
    pub class: Option<String>,
    pub attrs: Attrs,
}

impl DateRange {
    pub fn is_ad(&self) -> bool {
        self.class.as_deref() == Some("twitch-stitched-ad") || self.id.starts_with("stitched-ad")
    }
}

#[derive(Default)]
pub struct MediaPlaylist {
    pub target_duration: f64,
    pub media_sequence: u64,
    pub segments: Vec<Segment>,
    pub dateranges: Vec<DateRange>,
    pub prefetch: Vec<String>,
    pub tag_names: BTreeSet<String>,
}

impl MediaPlaylist {
    pub fn ad_segment_count(&self) -> usize {
        self.segments.iter().filter(|s| s.is_ad()).count()
    }

    pub fn ad_dateranges(&self) -> impl Iterator<Item = &DateRange> {
        self.dateranges.iter().filter(|d| d.is_ad())
    }

    pub fn newest_is_ad(&self) -> bool {
        self.segments.last().is_some_and(Segment::is_ad)
    }
}

pub fn parse_media(text: &str) -> Result<MediaPlaylist> {
    if !text.trim_start().starts_with("#EXTM3U") {
        bail!("not an m3u8 playlist: {}", text.chars().take(80).collect::<String>());
    }
    let mut pl = MediaPlaylist { target_duration: 2.0, ..Default::default() };
    let mut pending_inf: Option<(f64, String)> = None;
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if let Some(tag) = line.strip_prefix('#') {
            let (name, value) = tag.split_once(':').unwrap_or((tag, ""));
            pl.tag_names.insert(name.to_string());
            match name {
                "EXT-X-TARGETDURATION" => pl.target_duration = value.parse().unwrap_or(2.0),
                "EXT-X-MEDIA-SEQUENCE" => {
                    pl.media_sequence = value.parse().context("bad EXT-X-MEDIA-SEQUENCE")?
                }
                "EXTINF" => {
                    let (d, t) = value.split_once(',').unwrap_or((value, ""));
                    pending_inf = Some((d.parse().unwrap_or(0.0), t.to_string()));
                }
                "EXT-X-DATERANGE" => {
                    let attrs = parse_attrs(value);
                    pl.dateranges.push(DateRange {
                        id: attr(&attrs, "ID").unwrap_or("").to_string(),
                        class: attr(&attrs, "CLASS").map(str::to_string),
                        attrs,
                    });
                }
                "EXT-X-TWITCH-PREFETCH" => pl.prefetch.push(value.to_string()),
                _ => {}
            }
        } else if let Some((duration, title)) = pending_inf.take() {
            // The line is the segment URI; the spike only needs its position.
            let seq = pl.media_sequence + pl.segments.len() as u64;
            pl.segments.push(Segment { seq, duration, title });
        }
    }
    Ok(pl)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attrs_with_quoted_commas() {
        let a = parse_attrs(r#"ID="stitched-ad-1",CLASS="twitch-stitched-ad",DURATION=30.0,X-LIST="a,b""#);
        assert_eq!(attr(&a, "ID"), Some("stitched-ad-1"));
        assert_eq!(attr(&a, "DURATION"), Some("30.0"));
        assert_eq!(attr(&a, "X-LIST"), Some("a,b"));
    }

    #[test]
    fn media_playlist_ad_signals() {
        let text = "#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:100\n\
            #EXT-X-DATERANGE:ID=\"stitched-ad-1\",CLASS=\"twitch-stitched-ad\",START-DATE=\"2026-11-02T20:00:00Z\"\n\
            #EXTINF:2.000,live\nseg100.ts\n#EXTINF:2.001,Amazon\nseg101.ts\n";
        let pl = parse_media(text).unwrap();
        assert_eq!(pl.segments.len(), 2);
        assert_eq!(pl.segments[1].seq, 101);
        assert_eq!(pl.ad_segment_count(), 1);
        assert_eq!(pl.ad_dateranges().count(), 1);
        assert!(pl.newest_is_ad());
    }
}
