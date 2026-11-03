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
        tl.update(0, &primary, &[Some(&backup)]);
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
fn second_backup_covers_where_the_first_is_in_an_ad() {
    // The first backup has its own ad during the first 30 s of the primary's break; the second
    // backup is live throughout. Expect: real stream everywhere, no slate.
    let ad_start = parse_pdt("2026-11-02T20:00:31.787Z").unwrap();
    let mut tl = Timeline::new();
    for text in EMBED {
        let primary = parse(text);
        let mut first = backup_like(&primary);
        for s in &mut first.segments {
            let p = s.pdt.unwrap();
            s.ad = (ad_start..ad_start + 30_000).contains(&p);
        }
        let second = parse(&backup_like(&primary).segments.iter().fold(
            "#EXTM3U\n#EXT-X-TARGETDURATION:6\n#EXT-X-MEDIA-SEQUENCE:1\n".to_string(),
            |acc, s| acc + &format!("#EXT-X-PROGRAM-DATE-TIME:{}\n#EXTINF:2.000,live\nhttps://backup2.example/{}.ts\n", format_pdt(s.pdt.unwrap()), s.pdt.unwrap()),
        ));
        tl.update(0, &primary, &[Some(&first), Some(&second)]);
        let out = tl.render(0, &SLATES);
        assert!(!out.contains("slate") && !out.contains("Amazon"), "only real stream content");
    }
    let since = tl.origins_since(ad_start);
    let (b0, b1) = (since.get(&Origin::Backup(0)).copied().unwrap_or(0), since.get(&Origin::Backup(1)).copied().unwrap_or(0));
    assert!((14..=16).contains(&b1), "second backup fills the first's ad: {since:?}");
    assert!(b0 >= 14, "first backup covers the rest of the break: {since:?}");
    assert_eq!(since.get(&Origin::Slate), None);
}

#[test]
fn slates_fill_an_ad_without_backup() {
    let mut tl = Timeline::new();
    let (mut last_seq, mut last_disc) = (0, 0);
    let mut slates_seen = std::collections::BTreeSet::new();
    for text in EMBED {
        tl.update(0, &parse(text), &[]);
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
        tl.update(0, &parse(text), &[]);
        tl.update(1, &parse(&other), &[]);
    }
    let (a, b) = (tl.render(0, &SLATES), tl.render(1, &SLATES));
    assert_eq!(header(&a, "#EXT-X-MEDIA-SEQUENCE:"), header(&b, "#EXT-X-MEDIA-SEQUENCE:"));
    let times = |p: &str| p.lines().filter(|l| l.starts_with("#EXT-X-PROGRAM-DATE-TIME")).map(str::to_string).collect::<Vec<_>>();
    assert_eq!(times(&a), times(&b));
    assert!(b.contains("cdn2.example") && !b.contains("https://cdn.example/seg"));
}

/// A regular live playlist: `count` segments of `dur` seconds from `start`.
fn live_playlist(start: i64, count: i64, dur: f64, first_seq: i64) -> String {
    let mut text = format!("#EXTM3U\n#EXT-X-TARGETDURATION:6\n#EXT-X-MEDIA-SEQUENCE:{first_seq}\n");
    for i in 0..count {
        let t = start + (i as f64 * dur * 1000.0).round() as i64;
        text += &format!("#EXT-X-PROGRAM-DATE-TIME:{}\n#EXTINF:{dur:.3},live\nhttps://cdn.example/{t}.ts\n", format_pdt(t));
    }
    text
}

#[test]
fn every_ad_segment_becomes_exactly_one_slate() {
    // Simulated ads on a clean 2 s grid: 15 of 60 segments are ads; refresh one segment at a time.
    for dur in [2.0, 1.985] {
        let start = parse_pdt("2026-11-02T22:55:00.000Z").unwrap();
        let mut tl = Timeline::new();
        let mut slates = std::collections::BTreeSet::new();
        let mut lives = std::collections::BTreeSet::new();
        for step in 0..60 {
            let first = (step - 13).max(0);
            let mut pl = parse(&live_playlist(start + (first as f64 * dur * 1000.0).round() as i64, step - first + 1, dur, first));
            for s in &mut pl.segments {
                s.ad = (30..45).contains(&(s.seq as i64));
            }
            tl.update(0, &pl, &[]);
            let out = tl.render(0, &SLATES);
            let mut seq = header(&out, "#EXT-X-MEDIA-SEQUENCE:");
            for line in out.lines().filter(|l| l.starts_with("#EXTINF")) {
                if line.ends_with(",slate") { slates.insert(seq) } else { lives.insert(seq) };
                seq += 1;
            }
        }
        assert_eq!(slates.len(), 15, "one slate per ad segment ({dur} s segments)");
        let all: Vec<u64> = slates.union(&lives).copied().collect();
        assert!(all.windows(2).all(|w| w[1] == w[0] + 1), "no hole in the numbering ({dur} s)");
    }
}

#[test]
fn without_slates_the_gap_is_skipped() {
    let mut tl = Timeline::with_slates(false);
    for text in EMBED {
        tl.update(0, &parse(text), &[]);
        let out = tl.render(0, &SLATES);
        assert!(!out.contains("slate") && !out.contains("Amazon"));
        assert!(!out.contains("#EXT-X-DISCONTINUITY
"), "live to live needs no discontinuity");
    }
    // The break (~60 s) is simply missing from the timeline, which ends live again.
    assert_eq!(tl.last_origin(), Some(Origin::Primary));
    assert!(tl.origins_since(0).get(&Origin::Slate).is_none());
}

#[test]
fn simulated_ads_mark_live_segments() {
    let mut pl = parse(EMBED[0]);
    assert!(!pl.has_ads());
    pl.simulate_ads(10, 5);
    assert!(pl.segments.iter().any(|s| s.ad));
    assert!(pl.segments.iter().any(|s| !s.ad));
}
