//! Minimal line-based HLS parsing (copied from S1, plus segment URI, PDT and discontinuities).

use std::collections::HashMap;

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
    pub uri: String,
    pub pdt: Option<String>,
    pub discontinuity: bool,
}

impl Segment {
    /// Live content segments are titled "live"; stitched ads carry other titles.
    pub fn is_ad(&self) -> bool {
        !self.title.is_empty() && self.title != "live"
    }
}

pub struct MediaPlaylist {
    pub target_duration: f64,
    pub segments: Vec<Segment>,
}

pub fn parse_media(text: &str) -> Result<MediaPlaylist> {
    if !text.trim_start().starts_with("#EXTM3U") {
        bail!("not an m3u8 playlist: {}", text.chars().take(80).collect::<String>());
    }
    let mut target_duration = 2.0;
    let mut media_sequence = 0;
    let mut segments = Vec::new();
    let mut pending_inf: Option<(f64, String)> = None;
    let mut pdt = None;
    let mut discontinuity = false;
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if let Some(tag) = line.strip_prefix('#') {
            let (name, value) = tag.split_once(':').unwrap_or((tag, ""));
            match name {
                "EXT-X-TARGETDURATION" => target_duration = value.parse().unwrap_or(2.0),
                "EXT-X-MEDIA-SEQUENCE" => {
                    media_sequence = value.parse().context("bad EXT-X-MEDIA-SEQUENCE")?
                }
                "EXT-X-PROGRAM-DATE-TIME" => pdt = Some(value.to_string()),
                "EXT-X-DISCONTINUITY" => discontinuity = true,
                "EXTINF" => {
                    let (d, t) = value.split_once(',').unwrap_or((value, ""));
                    pending_inf = Some((d.parse().unwrap_or(0.0), t.to_string()));
                }
                _ => {}
            }
        } else if let Some((duration, title)) = pending_inf.take() {
            segments.push(Segment {
                seq: media_sequence + segments.len() as u64,
                duration,
                title,
                uri: line.to_string(),
                pdt: pdt.take(),
                discontinuity: std::mem::take(&mut discontinuity),
            });
        }
    }
    Ok(MediaPlaylist { target_duration, segments })
}
