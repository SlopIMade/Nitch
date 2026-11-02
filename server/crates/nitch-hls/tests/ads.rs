//! Ad detection and the output timeline, against playlists recorded in spike S1
//! (sanitized with scripts/sanitize-playlist.py).

use nitch_hls::media::{MediaPlaylist, parse_media};
use nitch_hls::time::{format_pdt, parse_pdt};
use nitch_hls::timeline::{Origin, Slates, Timeline};
use url::Url;

/// An `embed` ad break on tarik: two 30.235 s ads (32 ad segments) in the middle of live segments.
const EMBED: [&str; 6] = [
    include_str!("fixtures/embed-1-before-ad.m3u8"),
    include_str!("fixtures/embed-2-ad-start.m3u8"),
    include_str!("fixtures/embed-3-mid-ad.m3u8"),
    include_str!("fixtures/embed-4-ad-last-segment.m3u8"),
    include_str!("fixtures/embed-5-live-again.m3u8"),
    include_str!("fixtures/embed-6-after-ad.m3u8"),
];
const SLATES: Slates = Slates { ts: "/hls/slate.ts", fmp4_init: "/hls/slate-init.mp4", fmp4: "/hls/slate.m4s" };

fn base() -> Url {
    Url::parse("https://cdn.example/playlist.m3u8").unwrap()
}

fn parse(text: &str) -> MediaPlaylist {
    parse_media(text, &base()).unwrap()
}

/// A live-only playlist in the backup's style: same 2 s grid, different URLs.
fn backup_like(primary: &MediaPlaylist) -> MediaPlaylist {
    let first = primary.segments.first().unwrap().pdt.unwrap();
    let edge = primary.edge().unwrap();
    let mut text = "#EXTM3U\n#EXT-X-TARGETDURATION:6\n#EXT-X-MEDIA-SEQUENCE:1\n".to_string();
    let mut t = first;
    while t + 2000 <= edge {
        text += &format!("#EXT-X-PROGRAM-DATE-TIME:{}\n#EXTINF:2.000,live\nhttps://backup.example/{t}.ts\n", format_pdt(t));
        t += 2000;
    }
    parse(&text)
}

fn header(playlist: &str, tag: &str) -> u64 {
    playlist.lines().find_map(|l| l.strip_prefix(tag)?.parse().ok()).unwrap()
}

#[test]
fn detects_ad_segments_by_title_and_daterange() {
    let pl = parse(EMBED[1]);
    let titled = pl.segments.iter().filter(|s| s.title.starts_with("Amazon")).count();
    assert_eq!(titled, 3);
    assert_eq!(pl.segments.iter().filter(|s| s.ad).count(), titled);
    assert!(pl.has_ads());
    let start = parse_pdt("2026-11-02T20:00:31.787Z").unwrap();
    assert_eq!(pl.ad_break_until(), Some(start + 30_235));
}

#[test]
fn announces_ad_before_its_segments() {
    // The DATERANGE arrives while all segments are still live (S1: ~4 s early).
    let pl = parse(include_str!("fixtures/pbp-early-daterange.m3u8"));
    assert!(pl.segments.iter().all(|s| !s.ad));
    assert!(pl.has_ads());
    let (ad_start, _) = pl.ad_ranges()[0];
    assert!(ad_start >= pl.edge().unwrap());
}

#[test]
fn parses_fmp4_init_segments() {
    let pl = parse(include_str!("fixtures/site-short-ad.m3u8"));
    assert!(pl.segments.iter().all(|s| s.map.is_some()));
    let first_ad = pl.segments.iter().position(|s| s.ad).unwrap();
    assert!(pl.segments[first_ad].discontinuity);
    assert_ne!(pl.segments[0].map, pl.segments[first_ad].map, "ads come with their own init segment");
}

#[test]
fn backup_replaces_ads_seamlessly() {
    let mut tl = Timeline::new();
    let mut last_seq = 0;
    for (i, text) in EMBED.iter().enumerate() {
        let primary = parse(text);
        let backup = backup_like(&primary);
        tl.update(0, &primary, Some(&backup));
        let out = tl.render(0, &SLATES);
        assert!(!out.contains("Amazon"), "no ad segment in the output");
        assert!(!out.contains("slate"), "backup covers the whole break");
        assert!(!out.contains("#EXT-X-DISCONTINUITY\n"), "same transcode, no discontinuity needed");
        if i == 2 {
            assert!(out.contains("https://backup.example/"), "mid-break the output plays the backup");
        }
        let seq = header(&out, "#EXT-X-MEDIA-SEQUENCE:");
        assert!(seq >= last_seq);
        last_seq = seq;
    }
    assert_eq!(tl.last_origin(), Some(Origin::Primary), "back on the primary after the break");
}

#[test]
fn slates_fill_an_ad_without_backup() {
    let mut tl = Timeline::new();
    let (mut last_seq, mut last_disc) = (0, 0);
    let mut slates_seen = std::collections::BTreeSet::new();
    for text in EMBED {
        tl.update(0, &parse(text), None);
        let out = tl.render(0, &SLATES);
        assert!(!out.contains("Amazon"));
        let (seq, disc) = (header(&out, "#EXT-X-MEDIA-SEQUENCE:"), header(&out, "#EXT-X-DISCONTINUITY-SEQUENCE:"));
        assert!(seq >= last_seq && disc >= last_disc, "numbering never goes backwards");
        (last_seq, last_disc) = (seq, disc);
        // Remember which entries were slates (by their media sequence number).
        let mut n = seq;
        for line in out.lines() {
            if line.starts_with("#EXTINF") {
                if line.ends_with(",slate") {
                    slates_seen.insert(n);
                }
                n += 1;
            }
        }
        // Every slate segment (and the first live one after them) starts with a discontinuity.
        let slates_here = out.matches("\n/hls/slate.ts").count();
        let discs = out.matches("#EXT-X-DISCONTINUITY\n").count();
        assert!(discs >= slates_here, "{discs} discontinuities for {slates_here} slates");
    }
    // 2 × 30.235 s of ads ≈ 30 slate segments of 2 s.
    assert!((29..=32).contains(&slates_seen.len()), "got {} slates", slates_seen.len());
    assert_eq!(tl.last_origin(), Some(Origin::Primary), "live again after the break");
}

#[test]
fn renditions_share_numbering() {
    let mut tl = Timeline::new();
    for text in EMBED {
        let other = text.replace("cdn.example", "cdn2.example");
        tl.update(0, &parse(text), None);
        tl.update(1, &parse(&other), None);
    }
    let (a, b) = (tl.render(0, &SLATES), tl.render(1, &SLATES));
    assert_eq!(header(&a, "#EXT-X-MEDIA-SEQUENCE:"), header(&b, "#EXT-X-MEDIA-SEQUENCE:"));
    let times = |p: &str| p.lines().filter(|l| l.starts_with("#EXT-X-PROGRAM-DATE-TIME")).map(str::to_string).collect::<Vec<_>>();
    assert_eq!(times(&a), times(&b));
    assert!(b.contains("cdn2.example") && !b.contains("https://cdn.example/seg"));
}

#[test]
fn simulated_ads_mark_live_segments() {
    let mut pl = parse(EMBED[0]);
    assert!(!pl.has_ads());
    pl.simulate_ads(10, 5);
    assert!(pl.segments.iter().any(|s| s.ad));
    assert!(pl.segments.iter().any(|s| !s.ad));
}
