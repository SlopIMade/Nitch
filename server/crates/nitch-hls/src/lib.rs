//! HLS playlist handling for Nitch:
//! - master playlists: parsing and rewriting for clients (this file)
//! - [`media`]: media playlists with ad detection
//! - [`timeline`]: the ad-free output timeline (backup segments, slates)
//!
//! Segment URLs stay absolute, so clients load live segments straight from the CDN (D16).

pub mod media;
pub mod time;
pub mod timeline;

use anyhow::{Result, bail};
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

/// One rendition of a master playlist.
#[derive(Debug, Clone)]
pub struct Variant {
    /// Display name from the matching `EXT-X-MEDIA` (e.g. "720p60").
    pub name: String,
    pub resolution: Option<String>,
    pub bandwidth: u64,
    pub uri: Url,
}

#[derive(Debug, Clone)]
pub struct Master {
    /// The original playlist text, used for rewriting.
    pub text: String,
    pub variants: Vec<Variant>,
}

pub fn parse_master(text: &str, base: &Url) -> Result<Master> {
    let mut names = std::collections::HashMap::new(); // GROUP-ID -> NAME
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
                    uri: base.join(line)?,
                });
            }
        }
    }
    if variants.is_empty() {
        bail!("master playlist has no variants");
    }
    Ok(Master { text: text.to_string(), variants })
}

/// Rewrites a master playlist for clients: the n-th variant URI becomes `variant_uri(n)`.
/// Twitch-internal tags (`EXT-X-TWITCH-INFO`, which contains the viewer's IP) are dropped.
pub fn rewrite_master(master: &Master, variant_uri: impl Fn(usize) -> String) -> String {
    let mut out = String::new();
    let mut index = 0;
    let mut after_stream_inf = false;
    for line in master.text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if line.starts_with("#EXT-X-TWITCH-INFO") {
            continue;
        }
        if after_stream_inf && !line.starts_with('#') {
            out.push_str(&variant_uri(index));
            index += 1;
            after_stream_inf = false;
        } else {
            after_stream_inf |= line.starts_with("#EXT-X-STREAM-INF:");
            out.push_str(line);
        }
        out.push('\n');
    }
    out
}

/// Sanity check for media playlists before handing them to clients.
pub fn check_media(text: &str) -> Result<()> {
    if !text.trim_start().starts_with("#EXTM3U") {
        bail!("not an m3u8 playlist: {}", text.chars().take(80).collect::<String>());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MASTER: &str = "#EXTM3U\n\
        #EXT-X-TWITCH-INFO:NODE=\"x\",USER-IP=\"1.2.3.4\"\n\
        #EXT-X-MEDIA:TYPE=VIDEO,GROUP-ID=\"chunked\",NAME=\"1080p60 (source)\",AUTOSELECT=YES,DEFAULT=YES\n\
        #EXT-X-STREAM-INF:BANDWIDTH=8000000,RESOLUTION=1920x1080,CODECS=\"avc1.4D401F,mp4a.40.2\",VIDEO=\"chunked\"\n\
        https://video.example/v1/playlist/aaa.m3u8\n\
        #EXT-X-MEDIA:TYPE=VIDEO,GROUP-ID=\"720p60\",NAME=\"720p60\",AUTOSELECT=YES,DEFAULT=YES\n\
        #EXT-X-STREAM-INF:BANDWIDTH=3300000,RESOLUTION=1280x720,CODECS=\"avc1.4D401F,mp4a.40.2\",VIDEO=\"720p60\"\n\
        https://video.example/v1/playlist/bbb.m3u8\n";

    fn base() -> Url {
        Url::parse("https://usher.example/api/channel/hls/x.m3u8").unwrap()
    }

    #[test]
    fn parses_variants_with_names() {
        let m = parse_master(MASTER, &base()).unwrap();
        assert_eq!(m.variants.len(), 2);
        assert_eq!(m.variants[0].name, "1080p60 (source)");
        assert_eq!(m.variants[1].resolution.as_deref(), Some("1280x720"));
        assert_eq!(m.variants[1].uri.as_str(), "https://video.example/v1/playlist/bbb.m3u8");
    }

    #[test]
    fn rewrite_replaces_uris_and_drops_twitch_info() {
        let m = parse_master(MASTER, &base()).unwrap();
        let out = rewrite_master(&m, |i| format!("v{i}.m3u8"));
        assert!(!out.contains("USER-IP"));
        assert!(!out.contains("video.example"));
        assert!(out.contains("VIDEO=\"chunked\"\nv0.m3u8\n"));
        assert!(out.contains("VIDEO=\"720p60\"\nv1.m3u8\n"));
        assert!(out.contains("NAME=\"720p60\""));
    }

    #[test]
    fn attrs_with_quoted_commas() {
        let a = parse_attrs(r#"ID="x",CODECS="a,b",N=3"#);
        assert_eq!(attr(&a, "CODECS"), Some("a,b"));
        assert_eq!(attr(&a, "N"), Some("3"));
    }
}
