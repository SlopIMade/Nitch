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
    /// What does a session look like after sitting idle? One fetch, `wait` seconds of nothing,
    /// then a fetch every 2 s: segment count, ads, first/last timestamp vs. the clock.
    Idle {
        channel: String,
        #[arg(long, default_value_t = 80)]
        wait: u64,
    },
    /// Does a preroll run out without anyone fetching? Three fresh sessions side by side: A fetches
    /// the media playlist only at the end, B at the start and the end, C every 2 s.
    Preroll {
        channel: String,
        /// Seconds until the final fetch of A and B.
        #[arg(long, default_value_t = 90)]
        wait: u64,
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
        Cmd::Preroll { channel, wait } => preroll(&tw, &channel, wait).await,
        Cmd::Idle { channel, wait } => idle(&tw, &channel, wait).await,
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

async fn idle(tw: &twitch::Twitch, channel: &str, wait: u64) -> Result<()> {
    let token = tw.playback_token(channel, "site").await?;
    let master = tw.master_playlist(channel, &token).await?;
    let url = master.variants.iter().min_by_key(|v| v.bandwidth).unwrap().uri.clone();
    let now = || std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64;
    let show = |label: &str, text: &str| {
        let pl = hls::parse_media(text).unwrap();
        let pdts: Vec<i64> = text
            .lines()
            .filter_map(|l| l.strip_prefix("#EXT-X-PROGRAM-DATE-TIME:"))
            .filter_map(|t| chrono::DateTime::parse_from_rfc3339(t.trim()).ok())
            .map(|t| t.timestamp_millis())
            .collect();
        let behind = |t: Option<&i64>| t.map_or("-".to_string(), |t| format!("{:.1}s", (now() - t) as f64 / 1000.0));
        let gaps: Vec<String> = pdts.windows(2).filter(|w| w[1] - w[0] > 3000).map(|w| format!("{:.1}s", (w[1] - w[0]) as f64 / 1000.0)).collect();
        println!(
            "{label:<12} segs={:>2} ads={:>2} seq={} first={} behind, last={} behind, gaps={gaps:?}",
            pl.segments.len(),
            pl.ad_segment_count(),
            pl.media_sequence,
            behind(pdts.first()),
            behind(pdts.last())
        );
    };
    let (_, text) = tw.get_text(&url).await?;
    show("first", &text);
    // As the server: a check of the same (lowest) rendition after 70 s, then the rest of the wait.
    tokio::time::sleep(Duration::from_secs(70)).await;
    let (_, text) = tw.get_text(&url).await?;
    show("check", &text);
    tokio::time::sleep(Duration::from_secs(wait)).await;
    // After the wait: the highest rendition, never fetched before (as a player would).
    let url = master.variants.iter().max_by_key(|v| v.bandwidth).unwrap().uri.clone();
    for i in 0..12 {
        let (_, text) = tw.get_text(&url).await?;
        show(&format!("after +{}s", i * 2), &text);
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    Ok(())
}

async fn preroll(tw: &twitch::Twitch, channel: &str, wait: u64) -> Result<()> {
    // The cheapest rendition of a fresh "site" session.
    async fn session(tw: &twitch::Twitch, channel: &str) -> Result<url::Url> {
        let token = tw.playback_token(channel, "site").await?;
        let master = tw.master_playlist(channel, &token).await?;
        Ok(master.variants.iter().min_by_key(|v| v.bandwidth).unwrap().uri.clone())
    }
    async fn ads(tw: &twitch::Twitch, uri: &url::Url) -> Result<String> {
        let (_, text) = tw.get_text(uri).await?;
        let pl = hls::parse_media(&text)?;
        Ok(format!("ad {}/{}", pl.ad_segment_count(), pl.segments.len()))
    }
    let start = std::time::Instant::now();
    let t = |s: &std::time::Instant| format!("{:>6.1}s", s.elapsed().as_secs_f64());
    // B: the token's expiry, and its highest rendition (never fetched until the end).
    let token = tw.playback_token(channel, "site").await?;
    let expires = token.value.split("\"expires\":").nth(1).and_then(|r| r.split([',', '}']).next()).unwrap_or("?").to_string();
    let master = tw.master_playlist(channel, &token).await?;
    let b = master.variants.iter().min_by_key(|v| v.bandwidth).unwrap().uri.clone();
    let b_high = master.variants.iter().max_by_key(|v| v.bandwidth).unwrap().uri.clone();
    let (a, c) = (session(tw, channel).await?, session(tw, channel).await?);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs();
    println!("{} sessions A B C opened; B's token expires in {} s", t(&start), expires.trim().parse::<u64>().map(|e| e.saturating_sub(now) as i64).unwrap_or(-1));
    println!("{} B first fetch (lowest): {}", t(&start), ads(tw, &b).await?);
    let end = start + Duration::from_secs(wait);
    let mut c_done = false;
    while std::time::Instant::now() < end {
        let r = ads(tw, &c).await?;
        if !c_done && r.starts_with("ad 0/") {
            println!("{} C: preroll over ({r})", t(&start));
            c_done = true;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    println!("{} A first fetch (never fetched before): {}", t(&start), ads(tw, &a).await?);
    println!("{} B second fetch, lowest: {}", t(&start), ads(tw, &b).await?);
    println!("{} B first fetch, highest: {}", t(&start), ads(tw, &b_high).await?);
    // How long does B stay usable? Every 5 minutes, up to 30.
    for _ in 0..6 {
        tokio::time::sleep(Duration::from_secs(300)).await;
        let r = ads(tw, &b_high).await.map_err(|e| format!("{e:#}")).unwrap_or_else(|e| e);
        println!("{} B highest: {r}", t(&start));
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
