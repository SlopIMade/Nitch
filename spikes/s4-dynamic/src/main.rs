//! Spike S4: clock-paced multiview on a 60×60 grid with runtime changes.
//! Each tile is a `fallbacksrc` (live, re-timestamped, slate on failure); a live black background
//! and a live silence source drive the output clock, so no single tile can hold up the output.
//! A scripted scenario changes the layout, replaces tiles, breaks one, and switches audio.
//! Throwaway code, see docs/ROADMAP.md (M1). Findings go to FINDINGS.md.

#[allow(dead_code)] // shared copy from S3
mod hls;
#[allow(dead_code)]
mod twitch;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::Parser;
use gst::prelude::*;
use gstreamer as gst;

const OUT_W: i32 = 1920;
const OUT_H: i32 = 1080;
const FPS: i32 = 30;
/// The multiview grid (D17): 60×60 cells over the 16:9 canvas, 32×18 px each at 1080p.
const GRID: i32 = 60;
const DEAD_URI: &str = "http://127.0.0.1:9/offline.m3u8";

#[derive(Parser)]
#[command(about = "Spike S4: clock-paced multiview with runtime changes")]
struct Args {
    /// Six live channels: four to start, one to swap in, one for the late replacement.
    #[arg(long, value_delimiter = ',', required = true)]
    channels: Vec<String>,
    #[arg(long, default_value = "480p")]
    q: String,
    #[arg(long, default_value = "veryfast")]
    preset: String,
    #[arg(long, default_value = "/tmp/nitch-s4")]
    out: PathBuf,
    /// Where frame grabs go (e.g. a Windows-visible folder).
    #[arg(long)]
    frames: Option<PathBuf>,
    /// Skip the scenario: keep the 2×2 for this many seconds and report CPU per thread.
    #[arg(long)]
    hold: Option<u64>,
    /// Size of the live background that paces the compositor: "full" (1920×1080) or "tiny" (16×16).
    #[arg(long, default_value = "tiny")]
    bg: String,
}

/// A tile rectangle in grid cells.
#[derive(Clone, Copy)]
struct Rect {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

const fn r(x: i32, y: i32, w: i32, h: i32) -> Rect {
    Rect { x, y, w, h }
}

const QUAD: [Rect; 4] = [r(0, 0, 30, 30), r(30, 0, 30, 30), r(0, 30, 30, 30), r(30, 30, 30, 30)];
/// One big 40×40 tile (1280×720) and three 20×20 tiles (640×360) in a column on the right.
const BIG_PLUS_3: [Rect; 4] = [r(0, 0, 40, 40), r(40, 0, 20, 20), r(40, 20, 20, 20), r(40, 40, 20, 20)];

fn set_rect(pad: &gst::Pad, rect: Rect) {
    pad.set_property("xpos", rect.x * OUT_W / GRID);
    pad.set_property("ypos", rect.y * OUT_H / GRID);
    pad.set_property("width", rect.w * OUT_W / GRID);
    pad.set_property("height", rect.h * OUT_H / GRID);
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    anyhow::ensure!(args.channels.len() == 6, "give six channels");
    let tw = twitch::Twitch::new()?;
    let mut uris = Vec::new();
    for c in &args.channels {
        let token = tw.playback_token(c, "site").await?;
        let master = tw.master_playlist(c, &token).await?;
        let v = master
            .variants
            .iter()
            .find(|v| v.name.contains(&args.q))
            .with_context(|| format!("{c}: no rendition matching '{}'", args.q))?;
        println!("{c:<14} {}", v.name);
        uris.push(v.uri.to_string());
    }
    tokio::task::spawn_blocking(move || run(&args, &uris)).await?
}

fn make(factory: &str) -> Result<gst::Element> {
    Ok(gst::ElementFactory::make(factory).build()?)
}

struct Tile {
    elements: Vec<gst::Element>,
    comp_pad: gst::Pad,
    mix_pad: gst::Pad,
}

struct Mv {
    pipeline: gst::Pipeline,
    comp: gst::Element,
    mixer: gst::Element,
    slate_uri: String,
    tiles: Vec<Option<Tile>>,
    focus: usize,
}

impl Mv {
    fn add_tile(&mut self, i: usize, uri: &str, rect: Rect) -> Result<()> {
        let src = gst::ElementFactory::make("fallbacksrc")
            .name(format!("tile{i}-{}", self.pipeline.children().len()))
            .property("uri", uri)
            .property("fallback-uri", &self.slate_uri)
            .property("timeout", 3_000_000_000u64)
            .property("restart-timeout", 5_000_000_000u64)
            .property("retry-timeout", 3_600_000_000_000u64)
            .property("restart-on-eos", true)
            .property("immediate-fallback", true)
            .build()?;
        let aconv = make("audioconvert")?;
        let ares = make("audioresample")?;
        let acaps = gst::ElementFactory::make("capsfilter").property("caps", audio_caps()).build()?;
        self.pipeline.add_many([&src, &aconv, &ares, &acaps])?;
        gst::Element::link_many([&aconv, &ares, &acaps])?;

        let comp_pad = self.comp.request_pad_simple("sink_%u").context("compositor pad")?;
        comp_pad.set_property_from_str("sizing-policy", "keep-aspect-ratio");
        comp_pad.set_property("zorder", i as u32 + 1);
        set_rect(&comp_pad, rect);
        let mix_pad = self.mixer.request_pad_simple("sink_%u").context("mixer pad")?;
        mix_pad.set_property("volume", if i == self.focus { 1.0f64 } else { 0.0 });
        acaps.static_pad("src").unwrap().link(&mix_pad)?;

        let video_target = comp_pad.clone();
        let audio_target = aconv.static_pad("sink").unwrap();
        src.connect_pad_added(move |_, pad| {
            let name = pad.name();
            let target = if name.starts_with("video") { &video_target } else { &audio_target };
            match pad.link(target) {
                Ok(_) => println!("tile {i}: {name} linked"),
                Err(e) => eprintln!("tile {i}: linking {name} failed: {e:?}"),
            }
        });
        let last_status = std::sync::Mutex::new(String::new());
        src.connect_notify(Some("status"), move |s, _| {
            let status = s.property_value("status").serialize().map(|v| v.to_string()).unwrap_or_default();
            let mut last = last_status.lock().unwrap();
            if *last != status {
                println!("tile {i}: status {status}");
                *last = status;
            }
        });
        for e in [&aconv, &ares, &acaps, &src] {
            e.sync_state_with_parent()?;
        }
        self.tiles[i] = Some(Tile { elements: vec![src, aconv, ares, acaps], comp_pad, mix_pad });
        Ok(())
    }

    fn replace_tile(&mut self, i: usize, uri: &str, rect: Rect) -> Result<()> {
        if let Some(old) = self.tiles[i].take() {
            for e in &old.elements {
                e.set_state(gst::State::Null)?;
            }
            self.pipeline.remove_many(&old.elements)?;
            self.comp.release_request_pad(&old.comp_pad);
            self.mixer.release_request_pad(&old.mix_pad);
        }
        self.add_tile(i, uri, rect)
    }

    fn set_layout(&self, layout: &[Rect; 4]) {
        for (t, rect) in self.tiles.iter().zip(layout) {
            if let Some(t) = t {
                set_rect(&t.comp_pad, *rect);
            }
        }
    }

    fn set_focus(&mut self, i: usize) {
        self.focus = i;
        for (n, t) in self.tiles.iter().enumerate() {
            if let Some(t) = t {
                t.mix_pad.set_property("volume", if n == i { 1.0f64 } else { 0.0 });
            }
        }
    }
}

fn audio_caps() -> gst::Caps {
    gst::Caps::builder("audio/x-raw")
        .field("format", "S16LE")
        .field("rate", 48000)
        .field("channels", 2)
        .field("layout", "interleaved")
        .build()
}

fn make_slate(dir: &Path) -> Result<String> {
    let png = dir.join("slate.png");
    let status = Command::new("gst-launch-1.0")
        .args(["-q", "videotestsrc", "pattern=solid-color", "foreground-color=0xff202028", "num-buffers=1", "!"])
        .args(["video/x-raw,width=852,height=480", "!", "textoverlay", "text=Stream unavailable"])
        .args(["font-desc=Sans 28", "valignment=center", "halignment=center", "!", "videoconvert", "!"])
        .args(["pngenc", "!", "filesink", &format!("location={}", png.display())])
        .status()?;
    anyhow::ensure!(status.success(), "creating the slate failed");
    Ok(format!("file://{}", png.display()))
}

fn run(a: &Args, uris: &[String]) -> Result<()> {
    gst::init()?;
    gstfallbackswitch::plugin_register_static()?;
    let _ = std::fs::remove_dir_all(&a.out);
    std::fs::create_dir_all(&a.out)?;
    let frames = a.frames.clone().unwrap_or_else(|| a.out.join("frames"));
    std::fs::create_dir_all(&frames)?;
    let slate_uri = make_slate(&a.out)?;

    let pipeline = gst::Pipeline::new();
    let live_latency = 500_000_000u64;

    // Video output, driven by a live black background so the clock - not a tile - sets the pace.
    let comp = gst::ElementFactory::make("compositor")
        .property_from_str("background", "black")
        .property("ignore-inactive-pads", true)
        .property("latency", live_latency)
        .build()?;
    let bg = gst::ElementFactory::make("videotestsrc")
        .property("is-live", true)
        .property_from_str("pattern", "black")
        .build()?;
    let caps = |w: i32, h: i32| {
        gst::Caps::builder("video/x-raw")
            .field("width", w)
            .field("height", h)
            .field("framerate", gst::Fraction::new(FPS, 1))
            .build()
    };
    // The background only has to be live; a tiny one avoids blending a full frame every time.
    let bg_caps = if a.bg == "full" { caps(OUT_W, OUT_H) } else { caps(16, 16) };
    let bg_filter = gst::ElementFactory::make("capsfilter").property("caps", &bg_caps).build()?;
    // Pin the formats: left to negotiation, fallbacksrc's inputs made the compositor pick a 4:4:4
    // format and x264 encoded High 4:4:4 10-bit, which is slow and unplayable in browsers.
    let out_caps = gst::Caps::builder("video/x-raw")
        .field("format", "I420")
        .field("width", OUT_W)
        .field("height", OUT_H)
        .field("framerate", gst::Fraction::new(FPS, 1))
        .build();
    let out_filter = gst::ElementFactory::make("capsfilter").property("caps", &out_caps).build()?;
    let h264_caps = gst::Caps::builder("video/x-h264").field("profile", "high").build();
    let h264_filter = gst::ElementFactory::make("capsfilter").property("caps", &h264_caps).build()?;
    let conv = make("videoconvert")?;
    let vq = make("queue")?;
    let enc = gst::ElementFactory::make("x264enc")
        .property_from_str("speed-preset", &a.preset)
        .property_from_str("tune", "zerolatency")
        .property("bitrate", 6000u32)
        .property("key-int-max", (FPS * 2) as u32)
        .build()?;
    let parse = make("h264parse")?;
    let sink = gst::ElementFactory::make("hlssink2")
        .property("location", a.out.join("seg%05d.ts").to_str().unwrap())
        .property("playlist-location", a.out.join("index.m3u8").to_str().unwrap())
        .property("target-duration", 2u32)
        .property("playlist-length", 6u32)
        .property("max-files", 30u32)
        .build()?;
    pipeline.add_many([&bg, &bg_filter, &comp, &out_filter, &conv, &vq, &enc, &h264_filter, &parse, &sink])?;
    gst::Element::link_many([&bg, &bg_filter])?;
    let bg_pad = comp.request_pad_simple("sink_%u").unwrap();
    bg_pad.set_property("zorder", 0u32);
    bg_filter.static_pad("src").unwrap().link(&bg_pad)?;
    gst::Element::link_many([&comp, &out_filter, &conv, &vq, &enc, &h264_filter, &parse])?;
    parse.static_pad("src").unwrap().link(&sink.request_pad_simple("video").unwrap())?;

    // Audio output: live silence + every tile into audiomixer; only the focused tile is audible.
    let mixer = gst::ElementFactory::make("audiomixer")
        .property("ignore-inactive-pads", true)
        .property("latency", live_latency)
        .build()?;
    let silence = gst::ElementFactory::make("audiotestsrc")
        .property("is-live", true)
        .property_from_str("wave", "silence")
        .build()?;
    let silence_caps = gst::ElementFactory::make("capsfilter").property("caps", audio_caps()).build()?;
    let aconv = make("audioconvert")?;
    let aenc = gst::ElementFactory::make("avenc_aac").property("bitrate", 128_000i32).build()?;
    let aparse = make("aacparse")?;
    let aq = make("queue")?;
    pipeline.add_many([&silence, &silence_caps, &mixer, &aconv, &aenc, &aparse, &aq])?;
    gst::Element::link_many([&silence, &silence_caps])?;
    silence_caps.static_pad("src").unwrap().link(&mixer.request_pad_simple("sink_%u").unwrap())?;
    gst::Element::link_many([&mixer, &aconv, &aenc, &aparse, &aq])?;
    aq.static_pad("src").unwrap().link(&sink.request_pad_simple("audio").unwrap())?;

    let mut mv = Mv { pipeline: pipeline.clone(), comp, mixer, slate_uri, tiles: vec![None, None, None, None], focus: 0 };
    for i in 0..4 {
        mv.add_tile(i, &uris[i], QUAD[i])?;
    }

    pipeline.set_state(gst::State::Playing)?;
    let result = scenario(&mut mv, a, uris, &frames);
    pipeline.set_state(gst::State::Null)?;
    result
}

/// Newest complete segment index (the highest one may still be written).
fn newest_segment(dir: &Path) -> Option<u64> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.strip_prefix("seg")?.strip_suffix(".ts")?.parse::<u64>().ok())
        .max()
}

fn grab_frame(dir: &Path, frames: &Path, label: &str) {
    let Some(n) = newest_segment(dir).filter(|n| *n > 0) else { return };
    let seg = dir.join(format!("seg{:05}.ts", n - 1));
    let png = frames.join(format!("s4-{label}.png"));
    let ok = Command::new("gst-launch-1.0")
        .args(["-q", "filesrc", &format!("location={}", seg.display()), "!", "tsdemux", "!", "h264parse"])
        .args(["!", "avdec_h264", "!", "videoconvert", "!", "videoscale", "!", "video/x-raw,width=960,height=540"])
        .args(["!", "pngenc", "snapshot=true", "!", "filesink", &format!("location={}", png.display())])
        .status()
        .is_ok_and(|s| s.success());
    println!("frame {label}: {} from seg{:05}", if ok { "saved" } else { "FAILED" }, n - 1);
}

/// CPU seconds per thread name (GStreamer names streaming threads after their element/pad).
fn thread_cpu() -> std::collections::BTreeMap<String, f64> {
    let mut out = std::collections::BTreeMap::new();
    for task in std::fs::read_dir("/proc/self/task").into_iter().flatten().flatten() {
        let p = task.path();
        let (Ok(comm), Ok(stat)) = (std::fs::read_to_string(p.join("comm")), std::fs::read_to_string(p.join("stat")))
        else {
            continue;
        };
        let Some((_, rest)) = stat.rsplit_once(')') else { continue };
        let f: Vec<&str> = rest.split_whitespace().collect();
        let secs = (f[11].parse::<u64>().unwrap_or(0) + f[12].parse::<u64>().unwrap_or(0)) as f64 / 100.0;
        // Group numbered threads (e.g. x264 workers) under one name.
        let name = comm.trim().trim_end_matches(|c: char| c.is_ascii_digit() || c == ':').to_string();
        *out.entry(name).or_insert(0.0) += secs;
    }
    out
}

fn print_thread_cpu(before: &std::collections::BTreeMap<String, f64>, secs: f64) {
    let after = thread_cpu();
    let mut rows: Vec<(String, f64)> =
        after.iter().map(|(k, v)| (k.clone(), (v - before.get(k).copied().unwrap_or(0.0)) / secs)).collect();
    rows.sort_by(|a, b| b.1.total_cmp(&a.1));
    println!("CPU by thread (cores):");
    for (name, cores) in rows.iter().take(14).filter(|r| r.1 >= 0.02) {
        println!("  {cores:5.2}  {name}");
    }
}

fn cpu_seconds() -> Result<f64> {
    let stat = std::fs::read_to_string("/proc/self/stat")?;
    let f: Vec<&str> = stat.rsplit_once(')').context("bad stat")?.1.split_whitespace().collect();
    Ok((f[11].parse::<u64>()? + f[12].parse::<u64>()?) as f64 / 100.0)
}

fn scenario(mv: &mut Mv, a: &Args, uris: &[String], frames: &Path) -> Result<()> {
    let names = &a.channels;
    // (second, label, action)
    type Step<'a> = (u64, &'a str, Box<dyn Fn(&mut Mv) -> Result<()> + 'a>);
    let mut steps: Vec<Step> = vec![
        (20, "1-quad", Box::new(|_| Ok(()))),
        (25, "", Box::new(|mv| Ok(mv.set_layout(&BIG_PLUS_3)))),
        (35, "2-big-plus-3", Box::new(|_| Ok(()))),
        (40, "", Box::new(|mv| mv.replace_tile(1, &uris[4], BIG_PLUS_3[1]))),
        (52, "3-tile1-replaced", Box::new(|_| Ok(()))),
        (55, "", Box::new(|mv| mv.replace_tile(3, DEAD_URI, BIG_PLUS_3[3]))),
        (68, "4-tile3-dead", Box::new(|_| Ok(()))),
        (70, "", Box::new(|mv| {
            mv.set_layout(&QUAD);
            mv.replace_tile(3, &uris[5], QUAD[3])
        })),
        (85, "5-tile3-late-channel", Box::new(|_| Ok(()))),
        (90, "", Box::new(|mv| Ok(mv.set_focus(2)))),
        (100, "6-end", Box::new(|_| Ok(()))),
    ];
    if let Some(hold) = a.hold {
        steps = vec![(hold, "hold-end", Box::new(|_| Ok(())))];
    }
    let mut threads_at_20 = None;
    let describe = |s: u64| match s {
        25 => "layout -> 1 big + 3 small".to_string(),
        40 => format!("replace tile 1 with {}", names[4]),
        55 => "replace tile 3 with a dead URL".to_string(),
        70 => format!("layout -> 2x2, replace tile 3 with {}", names[5]),
        90 => format!("audio focus -> tile 2 ({})", names[2]),
        _ => String::new(),
    };

    let bus = mv.pipeline.bus().unwrap();
    let start = Instant::now();
    let mut next = 0;
    let mut last_seg: Option<(u64, Instant)> = None;
    let mut max_gap = Duration::ZERO;
    let mut first_seg_at = None;
    let cpu0 = cpu_seconds()?;
    let end = steps.last().unwrap().0 + 2;
    while start.elapsed() < Duration::from_secs(end) {
        while let Some(msg) = bus.timed_pop(gst::ClockTime::from_mseconds(100)) {
            let src = msg.src().map(|s| s.path_string().to_string()).unwrap_or_default();
            match msg.view() {
                gst::MessageView::Error(e) if src.contains("tile") => {
                    println!("{:>5.1}s  tile error (handled by fallbacksrc): {}", start.elapsed().as_secs_f64(), e.error())
                }
                gst::MessageView::Error(e) => bail!("{src}: {} ({:?})", e.error(), e.debug()),
                gst::MessageView::Warning(w) => println!("warning from {src}: {}", w.error()),
                gst::MessageView::Eos(..) => bail!("unexpected EOS"),
                _ => {}
            }
        }
        if let Some(n) = newest_segment(&a.out) {
            if last_seg.is_none_or(|(l, _)| n > l) {
                if let Some((_, at)) = last_seg {
                    max_gap = max_gap.max(at.elapsed());
                    if at.elapsed() > Duration::from_millis(3000) {
                        println!("{:>5.1}s  GAP: {:.1}s between segments", start.elapsed().as_secs_f64(), at.elapsed().as_secs_f64());
                    }
                }
                first_seg_at.get_or_insert(start.elapsed());
                last_seg = Some((n, Instant::now()));
            }
        }
        if threads_at_20.is_none() && start.elapsed() >= Duration::from_secs(20) {
            threads_at_20 = Some((Instant::now(), thread_cpu()));
        }
        if next < steps.len() && start.elapsed() >= Duration::from_secs(steps[next].0) {
            let (t, label, action) = &steps[next];
            if label.is_empty() {
                println!("{:>5.1}s  >> {}", start.elapsed().as_secs_f64(), describe(*t));
                action(mv)?;
            } else {
                grab_frame(&a.out, frames, label);
            }
            next += 1;
        }
    }
    let wall = start.elapsed().as_secs_f64();
    let segs = newest_segment(&a.out).unwrap_or(0) + 1;
    let since_first = wall - first_seg_at.unwrap_or_default().as_secs_f64() + 2.0;
    println!(
        "SUMMARY cpu {:.2} cores avg, {segs} segments = {}s video in {since_first:.0}s since first segment \
         ({:.2}x realtime), largest gap between segments {:.1}s",
        (cpu_seconds()? - cpu0) / wall,
        segs * 2,
        segs as f64 * 2.0 / since_first,
        max_gap.as_secs_f64()
    );
    if let Some((at, before)) = threads_at_20 {
        print_thread_cpu(&before, at.elapsed().as_secs_f64());
    }
    Ok(())
}
