//! Spike S3: 2×2 composite of live Twitch channels → x264 → HLS, with CPU measurement.
//! Throwaway code, see docs/ROADMAP.md (M1). Findings go to FINDINGS.md.

#[allow(dead_code)] // shared copy from S2
mod hls;
#[allow(dead_code)]
mod twitch;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::Parser;
use gst::prelude::*;
use gstreamer as gst;

const OUT_W: i32 = 1920;
const OUT_H: i32 = 1080;
/// Samples before this are startup (buffering, first keyframes) and not counted.
const WARMUP: Duration = Duration::from_secs(20);

#[derive(Parser)]
#[command(about = "Spike S3: 2x2 composite of live channels with CPU measurement")]
struct Args {
    /// 1 to 4 live channels, comma-separated.
    #[arg(long, value_delimiter = ',', required = true)]
    channels: Vec<String>,
    /// Input rendition per tile (substring match), e.g. 480p, 720p, 360p.
    #[arg(long, default_value = "480p")]
    q: String,
    /// x264 speed preset (ultrafast, superfast, veryfast, faster, fast, medium).
    #[arg(long, default_value = "veryfast")]
    preset: String,
    /// Output video bitrate in kbit/s.
    #[arg(long, default_value_t = 6000)]
    bitrate: u32,
    #[arg(long, default_value_t = 30)]
    fps: i32,
    #[arg(long, default_value_t = 90)]
    seconds: u64,
    #[arg(long, default_value = "/tmp/nitch-s3")]
    out: PathBuf,
    /// Shift each tile's timeline to the output position when its first frame arrives.
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    align_tiles: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    anyhow::ensure!((1..=4).contains(&args.channels.len()), "give 1 to 4 channels");
    let tw = twitch::Twitch::new()?;
    let mut urls = Vec::new();
    for c in &args.channels {
        let token = tw.playback_token(c, "site").await?;
        let master = tw.master_playlist(c, &token).await?;
        let v = master
            .variants
            .iter()
            .find(|v| v.name.contains(&args.q))
            .with_context(|| format!("{c}: no rendition matching '{}'", args.q))?;
        println!("{c:<14} {} {}", v.name, v.resolution.as_deref().unwrap_or("-"));
        urls.push(v.uri.to_string());
    }
    tokio::task::spawn_blocking(move || run(&args, &urls)).await?
}

fn make(factory: &str) -> Result<gst::Element> {
    Ok(gst::ElementFactory::make(factory).build()?)
}

fn running_time(pad: &gst::Pad, pts: gst::ClockTime) -> Option<gst::ClockTime> {
    let seg = pad.sticky_event::<gst::event::Segment>(0)?;
    seg.segment().downcast_ref::<gst::ClockTime>()?.to_running_time(pts)
}

fn run(a: &Args, urls: &[String]) -> Result<()> {
    gst::init()?;
    let _ = std::fs::remove_dir_all(&a.out);
    std::fs::create_dir_all(&a.out)?;
    let pipeline = gst::Pipeline::new();

    // Output: compositor → 1080p → x264 → hlssink2
    let comp = gst::ElementFactory::make("compositor")
        .property_from_str("background", "black")
        .property_from_str("start-time-selection", "first")
        .property("ignore-inactive-pads", true)
        .property("latency", 1_000_000_000u64)
        .build()?;
    let out_caps = gst::Caps::builder("video/x-raw")
        .field("width", OUT_W)
        .field("height", OUT_H)
        .field("framerate", gst::Fraction::new(a.fps, 1))
        .build();
    let out_filter = gst::ElementFactory::make("capsfilter").property("caps", &out_caps).build()?;
    let conv = make("videoconvert")?;
    let enc = gst::ElementFactory::make("x264enc")
        .property_from_str("speed-preset", &a.preset)
        .property_from_str("tune", "zerolatency")
        .property("bitrate", a.bitrate)
        .property("key-int-max", (a.fps * 2) as u32)
        .build()?;
    let parse = make("h264parse")?;
    let sink = gst::ElementFactory::make("hlssink2")
        .property("location", a.out.join("seg%05d.ts").to_str().unwrap())
        .property("playlist-location", a.out.join("index.m3u8").to_str().unwrap())
        .property("target-duration", 2u32)
        .property("playlist-length", 6u32)
        .property("max-files", 20u32)
        .build()?;
    pipeline.add_many([&comp, &out_filter, &conv, &enc, &parse, &sink])?;
    gst::Element::link_many([&comp, &out_filter, &conv, &enc, &parse])?;

    // Running time of the compositor's latest output frame, used to line up late tiles.
    let out_pos = Arc::new(AtomicU64::new(0));
    {
        let out_pos = out_pos.clone();
        comp.static_pad("src").unwrap().add_probe(gst::PadProbeType::BUFFER, move |pad, info| {
            if let Some(rt) = info.buffer().and_then(|b| running_time(pad, b.pts()?)) {
                out_pos.store(rt.nseconds(), Ordering::Relaxed);
            }
            gst::PadProbeReturn::Ok
        });
    }
    parse.static_pad("src").unwrap().link(&sink.request_pad_simple("video").unwrap())?;

    // Audio of tile 0 only; per-tile audio renditions come later.
    let aq = make("queue")?;
    let aconv = make("audioconvert")?;
    let ares = make("audioresample")?;
    let aenc = gst::ElementFactory::make("avenc_aac").property("bitrate", 128_000i32).build()?;
    let aparse = make("aacparse")?;
    pipeline.add_many([&aq, &aconv, &ares, &aenc, &aparse])?;
    gst::Element::link_many([&aq, &aconv, &ares, &aenc, &aparse])?;
    aparse.static_pad("src").unwrap().link(&sink.request_pad_simple("audio").unwrap())?;
    let audio_sink = aq.static_pad("sink").unwrap();

    // One tile per channel: uridecodebin3 → queue → scale to 960×540 → compositor pad.
    let (tile_w, tile_h) = (OUT_W / 2, OUT_H / 2);
    for (i, url) in urls.iter().enumerate() {
        let q = gst::ElementFactory::make("queue")
            .property("max-size-time", 4_000_000_000u64)
            .property("max-size-buffers", 0u32)
            .property("max-size-bytes", 0u32)
            .build()?;
        let scale = make("videoconvertscale")?;
        let tile_caps =
            gst::Caps::builder("video/x-raw").field("width", tile_w).field("height", tile_h).build();
        let tile_filter = gst::ElementFactory::make("capsfilter").property("caps", &tile_caps).build()?;
        pipeline.add_many([&q, &scale, &tile_filter])?;
        gst::Element::link_many([&q, &scale, &tile_filter])?;
        let comp_pad = comp.request_pad_simple("sink_%u").unwrap();
        comp_pad.set_property("xpos", (i as i32 % 2) * tile_w);
        comp_pad.set_property("ypos", (i as i32 / 2) * tile_h);
        let tile_src = tile_filter.static_pad("src").unwrap();
        tile_src.link(&comp_pad)?;
        // Report jumps in a tile's timeline (discontinuities, e.g. around ads).
        let last_rt = AtomicU64::new(u64::MAX);
        q.static_pad("src").unwrap().add_probe(gst::PadProbeType::BUFFER, move |pad, info| {
            if let Some(rt) = info.buffer().and_then(|b| running_time(pad, b.pts()?)) {
                let prev = last_rt.swap(rt.nseconds(), Ordering::Relaxed);
                let jump = rt.nseconds() as i64 - prev as i64;
                if prev != u64::MAX && !(-100_000_000..1_000_000_000).contains(&jump) {
                    println!("tile {i}: timeline jump {:+.2}s (now {rt})", jump as f64 / 1e9);
                }
            }
            gst::PadProbeReturn::Ok
        });
        // A tile that joins late starts at running time 0 while the output is already further on,
        // so all its frames would be "late" and dropped. On its first frame, shift the tile's
        // timeline to just ahead of the compositor's current output position.
        if a.align_tiles {
            let out_pos = out_pos.clone();
            q.static_pad("sink").unwrap().add_probe(gst::PadProbeType::BUFFER, move |pad, info| {
                let Some(rt) = info.buffer().and_then(|b| running_time(pad, b.pts()?)) else {
                    return gst::PadProbeReturn::Ok;
                };
                let target = out_pos.load(Ordering::Relaxed) + 500_000_000;
                let offset = target as i64 - rt.nseconds() as i64;
                tile_src.set_offset(offset);
                println!("tile {i}: first frame at running {rt}, offset {:+.2}s", offset as f64 / 1e9);
                gst::PadProbeReturn::Remove
            });
        }

        let src = gst::ElementFactory::make("uridecodebin3").property("uri", url).build()?;
        pipeline.add(&src)?;
        let video_sink = q.static_pad("sink").unwrap();
        let audio_sink = (i == 0).then(|| audio_sink.clone());
        let pipeline_weak = pipeline.downgrade();
        src.connect_pad_added(move |_, pad| {
            let name = pad.name();
            let wanted = if name.starts_with("video") {
                Some(video_sink.clone())
            } else if name.starts_with("audio") {
                audio_sink.clone()
            } else {
                None
            };
            let target = wanted.filter(|p| !p.is_linked()).or_else(|| {
                // Streams we don't use go to a fakesink, so the decoder never sees not-linked.
                let pl = pipeline_weak.upgrade()?;
                let fs = gst::ElementFactory::make("fakesink")
                    .property("sync", false)
                    .property("async", false)
                    .build()
                    .ok()?;
                pl.add(&fs).ok()?;
                fs.sync_state_with_parent().ok()?;
                fs.static_pad("sink")
            });
            match target.map(|t| pad.link(&t)) {
                Some(Ok(_)) => println!("tile {i}: {name} linked"),
                Some(Err(e)) => eprintln!("tile {i}: linking {name} failed: {e:?}"),
                None => eprintln!("tile {i}: no target for {name}"),
            }
        });
    }

    pipeline.set_state(gst::State::Playing)?;
    let result = measure(&pipeline, a);
    pipeline.set_state(gst::State::Null)?;
    result
}

/// Process CPU time (user + system) in seconds, from /proc/self/stat.
fn cpu_seconds() -> Result<f64> {
    let stat = std::fs::read_to_string("/proc/self/stat")?;
    let fields: Vec<&str> = stat.rsplit_once(')').context("bad /proc/self/stat")?.1.split_whitespace().collect();
    let ticks: u64 = fields[11].parse::<u64>()? + fields[12].parse::<u64>()?; // utime + stime
    Ok(ticks as f64 / 100.0) // CLK_TCK is 100 on Linux
}

/// Number of segments written so far (old ones get deleted, so use the highest index).
fn segments_written(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.strip_prefix("seg")?.strip_suffix(".ts")?.parse::<u64>().ok())
        .max()
        .map_or(0, |n| n + 1)
}

fn measure(pipeline: &gst::Pipeline, a: &Args) -> Result<()> {
    let bus = pipeline.bus().unwrap();
    let start = Instant::now();
    let mut last = (Instant::now(), cpu_seconds()?);
    let mut samples = Vec::new();
    let mut first_segment: Option<Duration> = None;
    while start.elapsed() < Duration::from_secs(a.seconds) {
        if let Some(msg) = bus.timed_pop(gst::ClockTime::from_mseconds(250)) {
            match msg.view() {
                gst::MessageView::Error(e) => bail!(
                    "{}: {} ({:?})",
                    e.src().map(|s| s.path_string().to_string()).unwrap_or_default(),
                    e.error(),
                    e.debug()
                ),
                gst::MessageView::Warning(w) => eprintln!("warning: {}", w.error()),
                gst::MessageView::Eos(..) => bail!("unexpected end of stream"),
                _ => {}
            }
        }
        if first_segment.is_none() && segments_written(&a.out) > 0 {
            first_segment = Some(start.elapsed());
        }
        if last.0.elapsed() >= Duration::from_secs(5) {
            let now = cpu_seconds()?;
            let cores = (now - last.1) / last.0.elapsed().as_secs_f64();
            println!("{:>4}s  cpu {cores:5.2} cores  segments {}", start.elapsed().as_secs(), segments_written(&a.out));
            if start.elapsed() > WARMUP {
                samples.push(cores);
            }
            last = (Instant::now(), now);
        }
    }

    let first = first_segment.context("no segment was written")?;
    let segments = segments_written(&a.out);
    // Segments are 2 s; compare video produced with wall time since the first segment.
    let produced = segments as f64 * 2.0;
    let wall = (start.elapsed() - first).as_secs_f64() + 2.0;
    let avg = samples.iter().sum::<f64>() / samples.len().max(1) as f64;
    let max = samples.iter().cloned().fold(0.0, f64::max);
    println!(
        "SUMMARY preset={} q={} fps={} bitrate={}k: cpu avg {avg:.2} / max {max:.2} cores, \
         first segment after {:.1}s, {segments} segments = {produced:.0}s video in {wall:.0}s ({:.2}x realtime)",
        a.preset,
        a.q,
        a.fps,
        a.bitrate,
        first.as_secs_f64(),
        produced / wall
    );
    Ok(())
}
