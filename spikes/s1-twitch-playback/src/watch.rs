//! Polls one media playlist, logs ad-break start/end and records playlists around each break.

use std::collections::{BTreeSet, HashSet, VecDeque};
use std::fs::{self, File};
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use chrono::Local;
use serde_json::{Map, Value, json};
use url::Url;

use crate::hls::{self, Attrs};
use crate::twitch::Twitch;

/// Playlists kept before an ad break and recorded after it.
const CONTEXT_SNAPSHOTS: usize = 3;
const HEARTBEAT: Duration = Duration::from_secs(60);

pub struct Options {
    pub channel: String,
    pub player_type: String,
    pub rendition: Option<String>,
    pub duration: Option<Duration>,
    pub record_dir: PathBuf,
}

/// Prints a short line per event and appends it as JSON to events.jsonl.
struct Events {
    file: File,
}

impl Events {
    fn log(&mut self, kind: &str, data: Value) {
        let now = Local::now();
        println!("{} {:<14} {}", now.format("%H:%M:%S"), kind, data);
        let line = json!({ "ts": now.to_rfc3339(), "kind": kind, "data": data });
        writeln!(self.file, "{line}").ok();
    }
}

fn attrs_json(attrs: &Attrs) -> Value {
    Value::Object(attrs.iter().map(|(k, v)| (k.clone(), Value::String(v.clone()))).collect::<Map<_, _>>())
}

pub async fn run(tw: &Twitch, o: Options) -> Result<()> {
    let run_dir = o.record_dir.join(format!("{}-{}", o.channel, Local::now().format("%Y%m%d-%H%M%S")));
    fs::create_dir_all(&run_dir).with_context(|| format!("creating {}", run_dir.display()))?;
    let mut ev = Events { file: File::create(run_dir.join("events.jsonl"))? };
    println!("recording to {}", run_dir.display());

    let started = Instant::now();
    let mut media_url = resolve(tw, &o, &mut ev).await?;

    let mut last_seq: Option<u64> = None;
    let mut seen_tags = BTreeSet::new();
    let mut seen_dateranges = HashSet::new();
    let mut seen_titles = HashSet::new();
    let mut ring: VecDeque<(u64, String)> = VecDeque::new();
    let mut tail_after_ad = 0;
    let mut snapshot_no = 0;
    let mut ad_since: Option<(Instant, u64)> = None;
    let (mut segments_total, mut ad_segments_total, mut breaks) = (0u64, 0u64, 0u32);
    let mut last_heartbeat = Instant::now();
    let mut save = |seq: u64, text: &str| {
        snapshot_no += 1;
        fs::write(run_dir.join(format!("{snapshot_no:04}-seq{seq}.m3u8")), text).ok();
    };

    loop {
        if o.duration.is_some_and(|d| started.elapsed() >= d) {
            break;
        }
        let mut interval = Duration::from_secs(1);
        match tw.get_text(&media_url).await {
            Err(e) => ev.log("fetch_error", json!(format!("{e:#}"))),
            Ok((status, _)) if !status.is_success() => {
                ev.log("playlist_error", json!({ "status": status.as_u16() }));
                match resolve(tw, &o, &mut ev).await {
                    Ok(url) => media_url = url,
                    Err(e) => {
                        ev.log("stopped", json!(format!("{e:#}")));
                        break;
                    }
                }
            }
            Ok((_, text)) => match hls::parse_media(&text) {
                Err(e) => ev.log("parse_error", json!(format!("{e:#}"))),
                Ok(pl) => {
                    interval = Duration::from_secs_f64((pl.target_duration / 2.0).max(1.0));
                    let newest = pl.segments.last().map_or(0, |s| s.seq);

                    let new_tags: Vec<_> =
                        pl.tag_names.iter().filter(|t| seen_tags.insert((*t).clone())).cloned().collect();
                    if !new_tags.is_empty() {
                        ev.log("tags", json!(new_tags));
                    }
                    for dr in &pl.dateranges {
                        if seen_dateranges.insert(dr.id.clone()) {
                            let kind = if dr.is_ad() { "ad_daterange" } else { "daterange" };
                            ev.log(kind, attrs_json(&dr.attrs));
                        }
                    }

                    // A sequence far behind the last one means the playlist was reset.
                    if last_seq.is_some_and(|l| newest + 10 < l) {
                        ev.log("seq_reset", json!({ "from": last_seq, "to": newest }));
                        last_seq = None;
                    }
                    for s in pl.segments.iter().filter(|s| last_seq.is_none_or(|l| s.seq > l)) {
                        segments_total += 1;
                        if s.is_ad() {
                            ad_segments_total += 1;
                            if seen_titles.insert(s.title.clone()) {
                                ev.log("ad_title", json!({ "title": s.title, "duration": s.duration }));
                            }
                        }
                    }
                    last_seq = Some(last_seq.map_or(newest, |l| l.max(newest)));

                    let in_ad = pl.newest_is_ad() || pl.ad_dateranges().next().is_some();
                    match (in_ad, ad_since) {
                        (true, None) => {
                            breaks += 1;
                            ad_since = Some((Instant::now(), ad_segments_total));
                            ev.log("ad_start", json!({
                                "newest_segment_is_ad": pl.newest_is_ad(),
                                "ad_dateranges": pl.ad_dateranges().count(),
                                "ad_segments_in_playlist": pl.ad_segment_count(),
                                "prefetch": pl.prefetch.len(),
                            }));
                            for (seq, text) in ring.drain(..) {
                                save(seq, &text);
                            }
                            save(newest, &text);
                        }
                        (true, Some(_)) => save(newest, &text),
                        (false, Some((since, ad_segs_before))) => {
                            ad_since = None;
                            ev.log("ad_end", json!({
                                "seconds": (since.elapsed().as_secs_f64() * 10.0).round() / 10.0,
                                "ad_segments": ad_segments_total - ad_segs_before,
                            }));
                            tail_after_ad = CONTEXT_SNAPSHOTS;
                            save(newest, &text);
                        }
                        (false, None) if tail_after_ad > 0 => {
                            tail_after_ad -= 1;
                            save(newest, &text);
                        }
                        (false, None) => {
                            ring.push_back((newest, text));
                            if ring.len() > CONTEXT_SNAPSHOTS {
                                ring.pop_front();
                            }
                        }
                    }

                    if last_heartbeat.elapsed() >= HEARTBEAT {
                        last_heartbeat = Instant::now();
                        ev.log("heartbeat", json!({
                            "seq": newest, "segments": segments_total,
                            "ad_segments": ad_segments_total, "in_ad": in_ad,
                        }));
                    }
                }
            },
        }
        tokio::select! {
            _ = tokio::time::sleep(interval) => {}
            _ = tokio::signal::ctrl_c() => break,
        }
    }

    ev.log("summary", json!({
        "minutes": (started.elapsed().as_secs_f64() / 6.0).round() / 10.0,
        "segments": segments_total,
        "ad_segments": ad_segments_total,
        "ad_breaks": breaks,
        "snapshots_saved": snapshot_no,
        "dir": run_dir.display().to_string(),
    }));
    Ok(())
}

async fn resolve(tw: &Twitch, o: &Options, ev: &mut Events) -> Result<Url> {
    let token = tw.playback_token(&o.channel, &o.player_type).await?;
    let master = tw.master_playlist(&o.channel, &token).await?;
    let names: Vec<_> = master.variants.iter().map(|v| v.name.as_str()).collect();
    let v = match &o.rendition {
        Some(r) => master
            .variants
            .iter()
            .find(|v| v.name.contains(r.as_str()))
            .with_context(|| format!("no rendition matching '{r}' in {names:?}"))?,
        None => &master.variants[0],
    };
    ev.log("resolved", json!({
        "player_type": o.player_type, "rendition": v.name, "renditions": names, "token": token.flags(),
    }));
    Ok(v.uri.clone())
}
