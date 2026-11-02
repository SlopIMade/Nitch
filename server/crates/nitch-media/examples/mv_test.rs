//! Manual test: runs a multiview with the given tile URIs for a while and reports what it wrote.
//!   cargo run -p nitch-media --example mv_test -- <out-dir> <seconds> <uri>...

use std::time::{Duration, Instant};

use nitch_media::{Config, Encoder, Multiview, TileSpec};

fn main() -> anyhow::Result<()> {
    tracing_subscriber_init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (dir, secs, uris) = (&args[0], args[1].parse::<u64>()?, &args[2..]);
    let mut mv = Multiview::start(Config {
        dir: dir.into(),
        width: 1920,
        height: 1080,
        fps: 30,
        video_kbps: 6000,
        slots: 4,
        encoder: Encoder::X264 { preset: "veryfast".into() },
    })?;
    // 2×2 on the 60×60 grid: 30×30 cells = 960×540 px.
    for (k, uri) in uris.iter().enumerate().take(4) {
        let (x, y) = ((k as i32 % 2) * 960, (k as i32 / 2) * 540);
        mv.set_tile(k, Some(&TileSpec { uri: uri.clone(), x, y, w: 960, h: 540, z: k as u32, cover: k == 3 }))?;
    }
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(secs) {
        std::thread::sleep(Duration::from_secs(5));
        println!("{:>3}s states {:?} failure {:?}", start.elapsed().as_secs(), mv.tile_states(), mv.failure());
        if start.elapsed() > Duration::from_secs(secs / 2) && uris.len() > 4 {
            // Halfway: move tile 0 to full size behind the others, and replace tile 1.
            mv.set_tile(0, Some(&TileSpec { uri: uris[0].clone(), x: 0, y: 0, w: 1920, h: 1080, z: 0, cover: false }))?;
            mv.set_tile(1, Some(&TileSpec { uri: uris[4].clone(), x: 960, y: 0, w: 960, h: 540, z: 1, cover: false }))?;
        }
    }
    let keep = std::env::var("KEEP").is_ok();
    if keep {
        std::mem::forget(mv);
    } else {
        mv.stop();
    }
    Ok(())
}

fn tracing_subscriber_init() {
    tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::new("nitch_media=debug")).init();
}
