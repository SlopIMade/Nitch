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

use std::path::{Path, PathBuf};
use std::sync::Once;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, bail};
use gst::prelude::*;
use gstreamer as gst;

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

#[derive(Debug, Clone)]
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

#[derive(Debug, Clone)]
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
}

/// A tile's source and place, in output pixels.
#[derive(Debug, Clone, PartialEq, Eq)]
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
}

struct Slot {
    mixer: gst::Element,
    source: Option<Source>,
}

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
        .property("target-duration", 2u32)
        .property("playlist-length", 6u32)
        .property("max-files", 15u32)
        .property("enable-program-date-time", true)
        .build()?)
}

const LIVE_LATENCY: u64 = 500_000_000;

impl Multiview {
    pub fn start(cfg: Config) -> Result<Self> {
        init()?;
        let _ = std::fs::remove_dir_all(&cfg.dir);
        std::fs::create_dir_all(&cfg.dir)?;
        let pipeline = gst::Pipeline::new();

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
        let conv = make("videoconvert")?;
        // ... and every remaining field is fixed after the converter: the CMAF muxer refuses any
        // caps change mid-stream, and a joining tile could otherwise change colorimetry.
        let enc_caps = gst::ElementFactory::make("capsfilter")
            .property(
                "caps",
                raw(cfg.width, cfg.height)
                    .field("format", "I420")
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
                .property("key-int-max", (cfg.fps * 2) as u32)
                .build()?,
            Encoder::Va => gst::ElementFactory::make("vah264enc")
                .property("bitrate", cfg.video_kbps)
                .property("key-int-max", (cfg.fps * 2) as u32)
                .build()?,
        };
        let h264_caps = gst::ElementFactory::make("capsfilter")
            .property("caps", gst::Caps::builder("video/x-h264").field("profile", "high").build())
            .build()?;
        let parse = make("h264parse")?;
        let vsink = cmaf_sink(&cfg.dir.join("video"))?;
        pipeline.add_many([&bg, &bg_caps, &comp, &out_caps, &conv, &enc_caps, &vq, &enc, &h264_caps, &parse, &vsink])?;
        gst::Element::link_many([&bg, &bg_caps])?;
        let bg_pad = comp.request_pad_simple("sink_%u").context("compositor pad")?;
        bg_pad.set_property("zorder", 0u32);
        bg_caps.static_pad("src").unwrap().link(&bg_pad)?;
        gst::Element::link_many([&comp, &out_caps, &conv, &enc_caps, &vq, &enc, &h264_caps, &parse, &vsink])?;

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
            slots.push(Slot { mixer, source: None });
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
    pub fn set_tile(&mut self, k: usize, spec: Option<&TileSpec>) -> Result<()> {
        anyhow::ensure!(k < self.slots.len(), "slot {k} out of range");
        match (spec, self.slots[k].source.as_ref()) {
            (Some(spec), Some(src)) if src.uri == spec.uri => {
                place(&src.comp_pad, spec);
                Ok(())
            }
            (spec, _) => {
                self.remove_source(k)?;
                if let Some(spec) = spec {
                    self.add_source(k, spec)?;
                }
                Ok(())
            }
        }
    }

    fn remove_source(&mut self, k: usize) -> Result<()> {
        if let Some(old) = self.slots[k].source.take() {
            for e in &old.elements {
                e.set_state(gst::State::Null)?;
            }
            self.pipeline.remove_many(&old.elements)?;
            self.comp.release_request_pad(&old.comp_pad);
            self.slots[k].mixer.release_request_pad(&old.mix_pad);
        }
        Ok(())
    }

    fn add_source(&mut self, k: usize, spec: &TileSpec) -> Result<()> {
        // No fallback image: it failed to negotiate inside fallbacksrc and kept retrying in the
        // background. A failed tile shows black; the client draws its state (loading,
        // unavailable) as an overlay from `tile_states`, sharper and localizable.
        let src = gst::ElementFactory::make("fallbacksrc")
            .property("uri", &spec.uri)
            .property("timeout", 3_000_000_000u64)
            .property("restart-timeout", 5_000_000_000u64)
            // Keep retrying for a day; huge values (u64::MAX/2) overflow inside fallbacksrc.
            .property("retry-timeout", 86_400_000_000_000u64)
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
        place(&comp_pad, spec);
        let mix_pad = self.slots[k].mixer.request_pad_simple("sink_%u").context("mixer pad")?;
        acaps.static_pad("src").unwrap().link(&mix_pad)?;

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
        self.slots[k].source = Some(Source {
            uri: spec.uri.clone(),
            elements: vec![src.clone(), aconv, ares, acaps],
            comp_pad,
            mix_pad,
            fallbacksrc: src,
        });
        Ok(())
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
fn place(pad: &gst::Pad, spec: &TileSpec) {
    pad.set_property("xpos", spec.x);
    pad.set_property("ypos", spec.y);
    pad.set_property("width", spec.w);
    pad.set_property("height", spec.h);
    pad.set_property("zorder", spec.z + 1);
}
