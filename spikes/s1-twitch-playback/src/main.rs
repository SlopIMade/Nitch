//! Spike S1: fetch Twitch live playlists anonymously and watch them for ad markers.
//! Throwaway code, see docs/ROADMAP.md (M1). Findings go to FINDINGS.md.

mod hls;
mod twitch;
mod watch;

use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(about = "Spike S1: Twitch playback and ad markers")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Show playback token flags and the renditions of a live channel.
    Info {
        channel: String,
        #[arg(long, default_value = "site")]
        player_type: String,
    },
    /// Request playlists with several player types and report which contain ad markers right now.
    Probe {
        channel: String,
        #[arg(
            long,
            value_delimiter = ',',
            default_value = "site,embed,popout,autoplay,picture-by-picture,frontpage"
        )]
        player_types: Vec<String>,
    },
    /// Follow one rendition's media playlist and log ad breaks.
    Watch {
        channel: String,
        #[arg(long, default_value = "site")]
        player_type: String,
        /// Rendition to follow, substring match on its name (e.g. "720p"). Default: the first (source).
        #[arg(long)]
        rendition: Option<String>,
        /// Stop after this many minutes. Default: run until Ctrl+C.
        #[arg(long)]
        minutes: Option<u64>,
        /// Directory for the event log and the playlists recorded around ad breaks.
        #[arg(long, default_value = "data/s1")]
        record: PathBuf,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let tw = twitch::Twitch::new()?;
    match cli.cmd {
        Cmd::Info { channel, player_type } => info(&tw, &channel, &player_type).await,
        Cmd::Probe { channel, player_types } => probe(&tw, &channel, &player_types).await,
        Cmd::Watch { channel, player_type, rendition, minutes, record } => {
            let opts = watch::Options {
                channel,
                player_type,
                rendition,
                duration: minutes.map(|m| Duration::from_secs(m * 60)),
                record_dir: record,
            };
            watch::run(&tw, opts).await
        }
    }
}

async fn info(tw: &twitch::Twitch, channel: &str, player_type: &str) -> Result<()> {
    let token = tw.playback_token(channel, player_type).await?;
    println!("token: {}", token.flags());
    let master = tw.master_playlist(channel, &token).await?;
    println!("renditions:");
    for v in &master.variants {
        println!(
            "  {:<20} {:>10} {:>6} kbps  {}",
            v.name,
            v.resolution.as_deref().unwrap_or("-"),
            v.bandwidth / 1000,
            v.codecs.as_deref().unwrap_or("")
        );
    }
    Ok(())
}

async fn probe(tw: &twitch::Twitch, channel: &str, player_types: &[String]) -> Result<()> {
    for pt in player_types {
        match probe_one(tw, channel, pt).await {
            Ok(line) => println!("{pt:<20} {line}"),
            Err(e) => println!("{pt:<20} error: {e:#}"),
        }
    }
    Ok(())
}

async fn probe_one(tw: &twitch::Twitch, channel: &str, player_type: &str) -> Result<String> {
    let token = tw.playback_token(channel, player_type).await?;
    let master = tw.master_playlist(channel, &token).await?;
    // The cheapest rendition is enough to see the markers.
    let v = master.variants.iter().min_by_key(|v| v.bandwidth).unwrap();
    let (_, text) = tw.get_text(&v.uri).await?;
    let pl = hls::parse_media(&text)?;
    Ok(format!(
        "ad segments {:>2}/{:<2} ad dateranges {}  token: {}",
        pl.ad_segment_count(),
        pl.segments.len(),
        pl.ad_dateranges().count(),
        token.flags()
    ))
}
