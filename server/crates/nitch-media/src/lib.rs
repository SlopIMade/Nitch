//! Multiview pipelines (M4): live tiles composited onto one 16:9 canvas, encoded to H.264, and
//! written as CMAF HLS with one audio rendition per tile slot (D7, D18). The structure is the
//! one proven in spike S4:
//!
//! ```text
//! tile k:  fallbacksrc ─► compositor.sink                       (grid rect, keep-aspect-ratio)
//!                     └─► audioconvert ─► audioresample ─► mixer_k
//! pacing:  videotestsrc is-live (16×16)  ─► compositor          the clock, never a tile, drives
//!          audiotestsrc is-live (silence) ─► mixer_k (each)     the output
//! output:  compositor ─► I420 ─► H.264 (High) ─► hlscmafsink    video/
//!          mixer_k ─► AAC ─► hlscmafsink                        audio{k}/
//! ```
//!
//! Every slot always has an audio rendition (silence when empty), so the master playlist never
//! changes while the pipeline runs.

pub mod clock;

use std::path::{Path, PathBuf};
use std::sync::Once;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, bail};
use gst::prelude::*;
use gstreamer as gst;
use serde::{Deserialize, Serialize};

/// Registers GStreamer and the statically linked Rust plugins. Safe to call repeatedly.
pub fn init() -> Result<()> {
    static ONCE: Once = Once::new();
    static RESULT: Mutex<Option<String>> = Mutex::new(None);
    ONCE.call_once(|| {
        let r = (|| -> Result<()> {
            gst::init()?;
            gstfallbackswitch::plugin_register_static()?;
            gsthlssink3::plugin_register_static()?;
            gstfmp4::plugin_register_static()?;
            Ok(())
        })();
        *RESULT.lock().unwrap() = r.err().map(|e| format!("{e:#}"));
    });
    match RESULT.lock().unwrap().as_ref() {
        Some(e) => bail!("GStreamer init failed: {e}"),
        None => Ok(()),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Encoder {
    /// Software, always available.
    X264 { preset: String },
    /// VA-API hardware encoding (target VM, M8).
    Va,
}

impl Encoder {
    /// The configured encoder if its element exists, otherwise x264.
    pub fn resolve(self) -> Encoder {
        match self {
            Encoder::Va if gst::ElementFactory::find("vah264enc").is_some() => Encoder::Va,
            Encoder::Va => {
                tracing::warn!("vah264enc not available, falling back to x264");
                Encoder::X264 { preset: "veryfast".into() }
            }
            other => other,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Output directory; gets `video/`, `audio{k}/` and `master.m3u8`.
    pub dir: PathBuf,
    pub width: i32,
    pub height: i32,
    pub fps: i32,
    pub video_kbps: u32,
    /// Number of tile slots (and audio renditions).
    pub slots: usize,
    pub encoder: Encoder,
    /// Wall-clock rate per monotonic second, as measured by the server so far (see `clock`).
    #[serde(default = "one")]
    pub clock_rate: f64,
}

fn one() -> f64 {
    1.0
}

/// A tile's source and place, in output pixels.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TileSpec {
    pub uri: String,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub z: u32,
    /// Crop the stream to fill the tile instead of letterboxing it.
    pub cover: bool,
}

struct Source {
    uri: String,
    elements: Vec<gst::Element>,
    comp_pad: gst::Pad,
    mix_pad: gst::Pad,
    fallbacksrc: gst::Element,
    started: std::time::Instant,
    /// When the first video frame reached the compositor. (fallbacksrc's `status` isn't
    /// enough: close to the live edge it reports "buffering" while frames flow.)
    first_frame: Arc<Mutex<Option<std::time::Instant>>>,
}

impl Source {
    /// How long frames have been arriving (None: not yet).
    fn showing_for(&self) -> Option<std::time::Duration> {
        self.first_frame.lock().unwrap().map(|t| t.elapsed())
    }
}

/// A hidden source takes over once it has delivered frames this long.
const SETTLE: std::time::Duration = std::time::Duration::from_millis(1500);

struct Slot {
    mixer: gst::Element,
    source: Option<Source>,
    /// A new source for the same tile (another rendition), started silent underneath the
    /// current one; it replaces `source` once it shows, so the picture never goes black (D30).
    pending: Option<(Source, TileSpec)>,
}

/// A hidden source that shows nothing by then is given up (the old one stays); shorter than
/// HIDDEN_TIMEOUT, after which it would start sending placeholder frames.
const PENDING_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(12);
/// fallbacksrc: no data for this long and the tile shows its placeholder (black, silent).
const TILE_TIMEOUT: u64 = 3_000_000_000;
/// The same for a hidden source, which mustn't take over with placeholder frames.
const HIDDEN_TIMEOUT: u64 = 15_000_000_000;

pub struct Multiview {
    cfg: Config,
    pipeline: gst::Pipeline,
    comp: gst::Element,
    slots: Vec<Slot>,
    stop: Arc<AtomicBool>,
    failed: Arc<Mutex<Option<String>>>,
}

fn make(factory: &str) -> Result<gst::Element> {
    gst::ElementFactory::make(factory).build().with_context(|| format!("creating {factory}"))
}

fn audio_caps() -> gst::Caps {
    gst::Caps::builder("audio/x-raw")
        .field("format", "S16LE")
        .field("rate", 48000)
        .field("channels", 2)
        .field("layout", "interleaved")
        .field("channel-mask", gst::Bitmask::new(0x3))
        .build()
}

/// One `hlscmafsink` writing into `dir`.
fn cmaf_sink(dir: &Path) -> Result<gst::Element> {
    std::fs::create_dir_all(dir)?;
    let p = |name: &str| dir.join(name).to_string_lossy().into_owned();
    Ok(gst::ElementFactory::make("hlscmafsink")
        .property("location", p("segment%05d.m4s"))
        .property("init-location", p("init%05d.mp4"))
        .property("playlist-location", p("index.m3u8"))
        // 1 s segments: players can stay closer to the live edge.
        .property("target-duration", SEGMENT_SECONDS)
        .property("playlist-length", 8u32)
        .property("max-files", 20u32)
        .property("enable-program-date-time", true)
        .build()?)
}

const LIVE_LATENCY: u64 = 500_000_000;
/// Output segment length; every segment starts with a keyframe.
const SEGMENT_SECONDS: u32 = 1;


impl Multiview {
    pub fn start(cfg: Config) -> Result<Self> {
        init()?;
        let _ = std::fs::remove_dir_all(&cfg.dir);
        std::fs::create_dir_all(&cfg.dir)?;
        let pipeline = gst::Pipeline::new();
        pipeline.use_clock(Some(&clock::steady_wall_clock(cfg.clock_rate)));

        // ---- video ----
        let comp = gst::ElementFactory::make("compositor")
            .property_from_str("background", "black")
            .property("ignore-inactive-pads", true)
            .property("latency", LIVE_LATENCY)
            .build()?;
        let bg = gst::ElementFactory::make("videotestsrc").property("is-live", true).property_from_str("pattern", "black").build()?;
        let raw = |w: i32, h: i32| {
            gst::Caps::builder("video/x-raw").field("width", w).field("height", h).field("framerate", gst::Fraction::new(cfg.fps, 1))
        };
        let bg_caps = gst::ElementFactory::make("capsfilter").property("caps", raw(16, 16).build()).build()?;
        // Pinned formats (S4): unpinned, the output became High 4:4:4 10-bit. The compositor only
        // gets format and size, so its inputs (including fallbacksrc's waiting image) still
        // negotiate freely ...
        let out_caps = gst::ElementFactory::make("capsfilter")
            .property("caps", raw(cfg.width, cfg.height).field("format", "I420").build())
            .build()?;
        // Strictly increasing timestamps at a constant rate: x264 aborts on a frame whose time
        // goes backwards, which a joining tile (latency recalculation) could otherwise cause.
        let rate = make("videorate")?;
        let conv = make("videoconvert")?;
        // ... and every remaining field is fixed after the converter: the CMAF muxer refuses any
        // caps change mid-stream, and a joining tile could otherwise change colorimetry.
        let enc_caps = gst::ElementFactory::make("capsfilter")
            .property(
                "caps",
                raw(cfg.width, cfg.height)
                    // What the encoder takes: the VA encoder only NV12, x264 I420.
                    .field("format", if matches!(cfg.encoder, Encoder::Va) { "NV12" } else { "I420" })
                    .field("pixel-aspect-ratio", gst::Fraction::new(1, 1))
                    .field("interlace-mode", "progressive")
                    .field("colorimetry", "bt709")
                    .field("chroma-site", "mpeg2")
                    .build(),
            )
            .build()?;
        let vq = make("queue")?;
        let enc = match &cfg.encoder {
            Encoder::X264 { preset } => gst::ElementFactory::make("x264enc")
                .property_from_str("speed-preset", preset)
                .property_from_str("tune", "zerolatency")
                .property("bitrate", cfg.video_kbps)
                .property("key-int-max", (cfg.fps * SEGMENT_SECONDS as i32) as u32)
                // Keyframes only every 2 s, never on scene cuts: even 2 s segments.
                .property("option-string", "scenecut=0")
                .build()?,
            Encoder::Va => gst::ElementFactory::make("vah264enc")
                .property("bitrate", cfg.video_kbps)
                .property("key-int-max", (cfg.fps * SEGMENT_SECONDS as i32) as u32)
                .build()?,
        };
        let h264_caps = gst::ElementFactory::make("capsfilter")
            .property("caps", gst::Caps::builder("video/x-h264").field("profile", "high").build())
            .build()?;
        let parse = make("h264parse")?;
        let vsink = cmaf_sink(&cfg.dir.join("video"))?;
        pipeline.add_many([&bg, &bg_caps, &comp, &out_caps, &rate, &conv, &enc_caps, &vq, &enc, &h264_caps, &parse, &vsink])?;
        gst::Element::link_many([&bg, &bg_caps])?;
        let bg_pad = comp.request_pad_simple("sink_%u").context("compositor pad")?;
        bg_pad.set_property("zorder", 0u32);
        bg_caps.static_pad("src").unwrap().link(&bg_pad)?;
        gst::Element::link_many([&comp, &out_caps, &rate, &conv, &enc_caps, &vq, &enc, &h264_caps, &parse, &vsink])?;

        // ---- audio, one rendition per slot ----
        let mut slots = Vec::new();
        for k in 0..cfg.slots {
            let silence = gst::ElementFactory::make("audiotestsrc").property("is-live", true).property_from_str("wave", "silence").build()?;
            let silence_caps = gst::ElementFactory::make("capsfilter").property("caps", audio_caps()).build()?;
            let mixer = gst::ElementFactory::make("audiomixer")
                .property("ignore-inactive-pads", true)
                .property("latency", LIVE_LATENCY)
                .build()?;
            let aconv = make("audioconvert")?;
            let mix_caps = gst::ElementFactory::make("capsfilter").property("caps", audio_caps()).build()?;
            let aenc = gst::ElementFactory::make("avenc_aac").property("bitrate", 128_000i32).build()?;
            let aparse = make("aacparse")?;
            let aq = make("queue")?;
            let asink = cmaf_sink(&cfg.dir.join(format!("audio{k}")))?;
            pipeline.add_many([&silence, &silence_caps, &mixer, &aconv, &mix_caps, &aenc, &aparse, &aq, &asink])?;
            gst::Element::link_many([&silence, &silence_caps])?;
            silence_caps.static_pad("src").unwrap().link(&mixer.request_pad_simple("sink_%u").context("mixer pad")?)?;
            gst::Element::link_many([&mixer, &mix_caps, &aconv, &aenc, &aparse, &aq, &asink])?;
            slots.push(Slot { mixer, source: None, pending: None });
        }

        let mv = Self {
            cfg,
            pipeline,
            comp,
            slots,
            stop: Arc::new(AtomicBool::new(false)),
            failed: Arc::new(Mutex::new(None)),
        };
        mv.write_master()?;
        mv.watch_bus();
        mv.pipeline.set_state(gst::State::Playing)?;
        Ok(mv)
    }

    /// Master playlist: one video rendition and one audio rendition per slot (`slot0` is default).
    fn write_master(&self) -> Result<()> {
        let mut m = String::from("#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-INDEPENDENT-SEGMENTS\n");
        for k in 0..self.cfg.slots {
            let default = if k == 0 { "YES" } else { "NO" };
            m += &format!(
                "#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"aud\",NAME=\"slot{k}\",LANGUAGE=\"und\",DEFAULT={default},AUTOSELECT={default},URI=\"audio{k}/index.m3u8\"\n"
            );
        }
        m += &format!(
            "#EXT-X-STREAM-INF:BANDWIDTH={},RESOLUTION={}x{},FRAME-RATE={},CODECS=\"avc1.640028,mp4a.40.2\",AUDIO=\"aud\"\nvideo/index.m3u8\n",
            (self.cfg.video_kbps + 128 * self.cfg.slots as u32) * 1000,
            self.cfg.width,
            self.cfg.height,
            self.cfg.fps
        );
        std::fs::write(self.cfg.dir.join("master.m3u8"), m)?;
        Ok(())
    }

    fn watch_bus(&self) {
        let bus = self.pipeline.bus().unwrap();
        let stop = self.stop.clone();
        let failed = self.failed.clone();
        let dir = self.cfg.dir.display().to_string();
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let Some(msg) = bus.timed_pop(gst::ClockTime::from_mseconds(500)) else { continue };
                let src = msg.src().map(|s| s.path_string().to_string()).unwrap_or_default();
                match msg.view() {
                    // Tile sources handle their own failures (fallback + retry).
                    gst::MessageView::Error(e) if src.contains("fallbacksrc") => {
                        tracing::debug!(%dir, %src, "tile source error: {}", e.error())
                    }
                    gst::MessageView::Error(e) => {
                        tracing::error!(%dir, %src, "multiview pipeline error: {} ({:?})", e.error(), e.debug());
                        *failed.lock().unwrap() = Some(e.error().to_string());
                    }
                    gst::MessageView::Warning(w) if !src.contains("fallbacksrc") => {
                        tracing::warn!(%dir, %src, "multiview pipeline warning: {}", w.error())
                    }
                    _ => {}
                }
            }
        });
    }

    pub fn dir(&self) -> &Path {
        &self.cfg.dir
    }

    pub fn slots(&self) -> usize {
        self.cfg.slots
    }

    /// Set when a non-tile element failed; the pipeline should be restarted.
    pub fn failure(&self) -> Option<String> {
        self.failed.lock().unwrap().clone()
    }

    /// Puts `spec` into slot `k` (or empties it). Same URI: only the geometry changes, live.
    /// Another URI while the current source runs (the same stream in another rendition): the
    /// new source starts hidden and takes over in [`Multiview::poll`] once it runs.
    pub fn set_tile(&mut self, k: usize, spec: Option<&TileSpec>) -> Result<()> {
        anyhow::ensure!(k < self.slots.len(), "slot {k} out of range");
        let Some(spec) = spec else {
            self.drop_pending(k)?;
            return self.remove_source(k);
        };
        let slot = &mut self.slots[k];
        if let Some((_, want)) = slot.pending.as_mut().filter(|(p, _)| p.uri == spec.uri) {
            *want = spec.clone();
            return Ok(());
        }
        match slot.source.as_ref() {
            Some(src) if src.uri == spec.uri => {
                place(&src.comp_pad, spec);
                self.drop_pending(k)
            }
            Some(src) if src.showing_for().is_some() => {
                // The layout changes now; the picture's quality once the new source shows.
                place(&src.comp_pad, spec);
                self.drop_pending(k)?;
                let hidden = self.make_source(k, spec, true)?;
                tracing::info!(slot = k, uri = %spec.uri, "starting another rendition hidden");
                self.slots[k].pending = Some((hidden, spec.clone()));
                Ok(())
            }
            _ => {
                self.drop_pending(k)?;
                self.remove_source(k)?;
                self.slots[k].source = Some(self.make_source(k, spec, false)?);
                Ok(())
            }
        }
    }

    /// Swaps in hidden sources that run now, gives up on those that never started. Call
    /// regularly (the worker does, every second).
    pub fn poll(&mut self) -> Result<()> {
        for k in 0..self.slots.len() {
            let Some((pending, _)) = self.slots[k].pending.as_ref() else { continue };
            if pending.showing_for().is_some_and(|d| d >= SETTLE) {
                let (new, spec) = self.slots[k].pending.take().unwrap();
                new.mix_pad.set_property("mute", false);
                new.fallbacksrc.set_property("timeout", TILE_TIMEOUT);
                place(&new.comp_pad, &spec);
                // The old one was on top of it; without it, the new one is simply visible.
                self.remove_source(k)?;
                tracing::info!(slot = k, uri = %new.uri, "tile switched to another rendition");
                self.slots[k].source = Some(new);
            } else if pending.started.elapsed() > PENDING_TIMEOUT {
                tracing::warn!(slot = k, uri = %pending.uri, "new rendition didn't start; keeping the old one");
                self.drop_pending(k)?;
            }
        }
        Ok(())
    }

    fn drop_pending(&mut self, k: usize) -> Result<()> {
        if let Some((old, _)) = self.slots[k].pending.take() {
            self.release(old)?;
        }
        Ok(())
    }

    fn remove_source(&mut self, k: usize) -> Result<()> {
        if let Some(old) = self.slots[k].source.take() {
            self.release(old)?;
        }
        Ok(())
    }

    fn release(&mut self, old: Source) -> Result<()> {
        for e in &old.elements {
            e.set_state(gst::State::Null)?;
        }
        self.pipeline.remove_many(&old.elements)?;
        self.comp.release_request_pad(&old.comp_pad);
        // The mixer belongs to the slot the pad came from.
        if let Some(mixer) = old.mix_pad.parent_element() {
            mixer.release_request_pad(&old.mix_pad);
        }
        Ok(())
    }

    /// A tile source for slot `k`, linked into the compositor and the slot's mixer; `hidden`
    /// ones are transparent and muted.
    fn make_source(&mut self, k: usize, spec: &TileSpec, hidden: bool) -> Result<Source> {
        // No fallback image: it failed to negotiate inside fallbacksrc and kept retrying in the
        // background. A failed tile shows black; the client draws its state (loading,
        // unavailable) as an overlay from `tile_states`, sharper and localizable.
        let src = gst::ElementFactory::make("fallbacksrc")
            .property("uri", &spec.uri)
            // Hidden (a rendition switch): no placeholder frames for a while, so the first frame that
            // arrives is the stream itself; normal again once it takes over (`poll`).
            .property("timeout", if hidden { HIDDEN_TIMEOUT } else { TILE_TIMEOUT })
            .property("restart-timeout", 5_000_000_000u64)
            // Keep retrying for a day; huge values (u64::MAX/2) overflow inside fallbacksrc.
            .property("retry-timeout", 86_400_000_000_000u64)
            .property("restart-on-eos", true)
            .property("immediate-fallback", !hidden)
            .build()?;
        let aconv = make("audioconvert")?;
        let ares = make("audioresample")?;
        let acaps = gst::ElementFactory::make("capsfilter").property("caps", audio_caps()).build()?;
        self.pipeline.add_many([&src, &aconv, &ares, &acaps])?;
        gst::Element::link_many([&aconv, &ares, &acaps])?;

        let comp_pad = self.comp.request_pad_simple("sink_%u").context("compositor pad")?;
        comp_pad.set_property_from_str("sizing-policy", "keep-aspect-ratio");
        place(&comp_pad, spec);
        let mix_pad = self.slots[k].mixer.request_pad_simple("sink_%u").context("mixer pad")?;
        if hidden {
            // Hidden by the old source on top of it, not by transparency: a transparent pad's
            // frames weren't composited, and after the swap the tile stayed black for seconds.
            comp_pad.set_property("zorder", zorder(spec.z) - 1);
            mix_pad.set_property("mute", true);
        }
        acaps.static_pad("src").unwrap().link(&mix_pad)?;

        let first_frame = Arc::new(Mutex::new(None));
        let flag = first_frame.clone();
        comp_pad.add_probe(gst::PadProbeType::BUFFER, move |_, _| {
            *flag.lock().unwrap() = Some(std::time::Instant::now());
            gst::PadProbeReturn::Remove
        });

        let video_target = comp_pad.clone();
        let audio_target = aconv.static_pad("sink").unwrap();
        src.connect_pad_added(move |_, pad| {
            let target = if pad.name().starts_with("video") { &video_target } else { &audio_target };
            if let Err(e) = pad.link(target) {
                tracing::warn!(slot = k, pad = %pad.name(), "linking tile pad failed: {e:?}");
            }
        });
        for e in [&aconv, &ares, &acaps, &src] {
            e.sync_state_with_parent()?;
        }
        Ok(Source {
            uri: spec.uri.clone(),
            elements: vec![src.clone(), aconv, ares, acaps],
            comp_pad,
            mix_pad,
            fallbacksrc: src,
            started: std::time::Instant::now(),
            first_frame,
        })
    }

    /// `fallbacksrc` status per slot (`running`, `buffering`, `retrying`, …), `None` when empty.
    pub fn tile_states(&self) -> Vec<Option<String>> {
        self.slots
            .iter()
            .map(|s| {
                s.source.as_ref().map(|src| {
                    src.fallbacksrc.property_value("status").serialize().map(|v| v.to_string()).unwrap_or_default()
                })
            })
            .collect()
    }

    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.pipeline.set_state(gst::State::Null);
        let _ = std::fs::remove_dir_all(&self.cfg.dir);
    }
}

impl Drop for Multiview {
    fn drop(&mut self) {
        if !self.stop.load(Ordering::Relaxed) {
            self.shutdown();
        }
    }
}

/// Applies a tile's geometry: position, size and stacking on the compositor pad.
/// "Cover" (crop to fill) isn't implemented yet: `aspectratiocrop` in front of the compositor
/// broke fallbacksrc's negotiation, so cover tiles are letterboxed like "contain" for now.
/// Compositor stacking for a tile: above the background (0), with room for a hidden source
/// right below each tile.
fn zorder(z: u32) -> u32 {
    (z + 1) * 2
}

fn place(pad: &gst::Pad, spec: &TileSpec) {
    pad.set_property("xpos", spec.x);
    pad.set_property("ypos", spec.y);
    pad.set_property("width", spec.w);
    pad.set_property("height", spec.h);
    pad.set_property("zorder", zorder(spec.z));
}
