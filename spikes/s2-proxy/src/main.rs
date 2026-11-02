//! Spike S2: playlist proxy + hls.js test page, and a site/embed timeline + CORS check.
//! Throwaway code, see docs/ROADMAP.md (M1). Findings go to FINDINGS.md.

mod hls;
mod rewrite;
mod twitch;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use clap::{Parser, Subcommand};
use serde_json::json;
use url::Url;

use hls::Variant;
use rewrite::{Mode, Rewriter};
use twitch::Twitch;

#[derive(Parser)]
#[command(about = "Spike S2: playlist proxy and timeline check")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Serve the test page and the rewriting proxy.
    Serve {
        #[arg(long, default_value_t = 8787)]
        port: u16,
    },
    /// Compare the timelines of two player types and check CORS on the Twitch CDN.
    Align {
        channel: String,
        #[arg(long, default_value = "480p")]
        q: String,
        #[arg(long, default_value = "embed")]
        other: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let tw = Twitch::new()?;
    match cli.cmd {
        Cmd::Serve { port } => serve(tw, port).await,
        Cmd::Align { channel, q, other } => align(&tw, &channel, &q, &other).await,
    }
}

// ---------- proxy ----------

struct App {
    tw: Twitch,
    streams: Mutex<HashMap<String, Stream>>,
}

struct Stream {
    variant_url: Url,
    rw: Rewriter,
}

/// What the page asked for; the same query string is passed on to the media playlist.
struct Opts {
    channel: String,
    pt: String,
    q: String,
    mode: Mode,
    proxy_segments: bool,
}

impl Opts {
    fn new(channel: String, p: &HashMap<String, String>) -> Self {
        let get = |k: &str, d: &str| p.get(k).cloned().unwrap_or_else(|| d.to_string());
        Self {
            channel,
            pt: get("pt", "site"),
            q: get("q", ""),
            mode: Mode::parse(&get("mode", "drop")),
            proxy_segments: get("seg", "direct") == "proxy",
        }
    }

    fn key(&self) -> String {
        format!("{}|{}|{}|{:?}|{}", self.channel, self.pt, self.q, self.mode, self.proxy_segments)
    }

    fn query(&self) -> String {
        let mode = format!("{:?}", self.mode).to_lowercase();
        let seg = if self.proxy_segments { "proxy" } else { "direct" };
        format!("pt={}&q={}&mode={mode}&seg={seg}", self.pt, self.q)
    }
}

struct AppError(anyhow::Error);

impl<E: Into<anyhow::Error>> From<E> for AppError {
    fn from(e: E) -> Self {
        Self(e.into())
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        eprintln!("error: {:#}", self.0);
        (StatusCode::BAD_GATEWAY, format!("{:#}", self.0)).into_response()
    }
}

fn m3u8(body: String) -> Response {
    ([(header::CONTENT_TYPE, "application/vnd.apple.mpegurl"), (header::CACHE_CONTROL, "no-cache")], body)
        .into_response()
}

async fn serve(tw: Twitch, port: u16) -> Result<()> {
    let app = Arc::new(App { tw, streams: Mutex::new(HashMap::new()) });
    let router = Router::new()
        .route("/", get(|| async { Html(include_str!("index.html")) }))
        .route("/play/{channel}/master.m3u8", get(master))
        .route("/play/{channel}/media.m3u8", get(media))
        .route("/seg", get(segment))
        .route("/state", get(state))
        .with_state(app);
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port)).await?;
    println!("listening on http://localhost:{port}");
    axum::serve(listener, router).await?;
    Ok(())
}

async fn pick_variant(tw: &Twitch, channel: &str, pt: &str, q: &str) -> Result<Variant> {
    let token = tw.playback_token(channel, pt).await?;
    let master = tw.master_playlist(channel, &token).await?;
    let names: Vec<_> = master.variants.iter().map(|v| v.name.clone()).collect();
    master
        .variants
        .into_iter()
        .find(|v| v.name.contains(q))
        .with_context(|| format!("no rendition matching '{q}' in {names:?}"))
}

/// Master playlist with a single variant, so the spike doesn't have to align renditions.
async fn master(
    State(app): State<Arc<App>>,
    Path(channel): Path<String>,
    Query(p): Query<HashMap<String, String>>,
) -> Result<Response, AppError> {
    let o = Opts::new(channel, &p);
    let v = pick_variant(&app.tw, &o.channel, &o.pt, &o.q).await?;
    println!("[{}] master -> {}", o.key(), v.name);
    let mut inf = format!("BANDWIDTH={}", v.bandwidth);
    if let Some(r) = &v.resolution {
        inf += &format!(",RESOLUTION={r}");
    }
    if let Some(c) = &v.codecs {
        inf += &format!(",CODECS=\"{c}\"");
    }
    let body = format!("#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-STREAM-INF:{inf}\nmedia.m3u8?{}\n", o.query());
    // A new master request starts a fresh output timeline.
    app.streams
        .lock()
        .unwrap()
        .insert(o.key(), Stream { variant_url: v.uri, rw: Rewriter::default() });
    Ok(m3u8(body))
}

async fn media(
    State(app): State<Arc<App>>,
    Path(channel): Path<String>,
    Query(p): Query<HashMap<String, String>>,
) -> Result<Response, AppError> {
    let o = Opts::new(channel, &p);
    let key = o.key();
    let known = app.streams.lock().unwrap().get(&key).map(|s| s.variant_url.clone());
    let mut url = match known {
        Some(u) => u,
        None => pick_variant(&app.tw, &o.channel, &o.pt, &o.q).await?.uri,
    };
    let (mut status, mut text) = app.tw.get_text(&url).await?;
    if !status.is_success() {
        println!("[{key}] upstream {status}, resolving again");
        url = pick_variant(&app.tw, &o.channel, &o.pt, &o.q).await?.uri;
        (status, text) = app.tw.get_text(&url).await?;
        if !status.is_success() {
            return Err(anyhow::anyhow!("upstream media playlist returned {status}").into());
        }
    }
    let pl = hls::parse_media(&text)?;
    let mut streams = app.streams.lock().unwrap();
    let s = streams.entry(key.clone()).or_insert_with(|| Stream { variant_url: url.clone(), rw: Rewriter::default() });
    s.variant_url = url;
    for e in s.rw.update(&pl, o.mode) {
        println!("[{key}] {e}");
    }
    Ok(m3u8(s.rw.render(o.proxy_segments)))
}

async fn segment(State(app): State<Arc<App>>, Query(p): Query<HashMap<String, String>>) -> Result<Response, AppError> {
    let u = p.get("u").context("missing u")?;
    let resp = app.tw.http().get(u).send().await?.error_for_status()?;
    let bytes = resp.bytes().await?;
    Ok(([(header::CONTENT_TYPE, "video/mp2t")], bytes).into_response())
}

async fn state(State(app): State<Arc<App>>) -> Response {
    let streams = app.streams.lock().unwrap();
    let list: Vec<_> = streams
        .iter()
        .map(|(k, s)| json!({ "key": k, "ad_now": s.rw.ad_now, "dropped": s.rw.dropped, "kept": s.rw.kept_total }))
        .collect();
    axum::Json(list).into_response()
}

// ---------- align ----------

async fn align(tw: &Twitch, channel: &str, q: &str, other: &str) -> Result<()> {
    let (a, b) = tokio::try_join!(pick_variant(tw, channel, "site", q), pick_variant(tw, channel, other, q))?;
    println!("rendition: site={} {other}={}", a.name, b.name);
    let ((_, ta), (_, tb)) = tokio::try_join!(tw.get_text(&a.uri), tw.get_text(&b.uri))?;
    let (pa, pb) = (hls::parse_media(&ta)?, hls::parse_media(&tb)?);

    let describe = |label: &str, pl: &hls::MediaPlaylist| {
        let (f, l) = (pl.segments.first().unwrap(), pl.segments.last().unwrap());
        println!(
            "{label:<6} seq {}..{}  pdt {} .. {}",
            f.seq,
            l.seq,
            f.pdt.as_deref().unwrap_or("-"),
            l.pdt.as_deref().unwrap_or("-")
        );
    };
    describe("site", &pa);
    describe(other, &pb);

    let pdts_b: HashMap<&str, &hls::Segment> =
        pb.segments.iter().filter_map(|s| s.pdt.as_deref().map(|p| (p, s))).collect();
    let common: Vec<(&hls::Segment, &hls::Segment)> = pa
        .segments
        .iter()
        .filter_map(|s| s.pdt.as_deref().and_then(|p| pdts_b.get(p)).map(|o| (s, *o)))
        .collect();
    let same_seq = common.iter().filter(|(x, y)| x.seq == y.seq).count();
    println!(
        "segments with identical PROGRAM-DATE-TIME: {}/{}  (same sequence number: {same_seq})",
        common.len(),
        pa.segments.len()
    );
    let ua: HashSet<_> = pa.segments.iter().map(|s| s.uri.as_str()).collect();
    println!("identical segment URLs: {}", pb.segments.iter().filter(|s| ua.contains(s.uri.as_str())).count());

    if let Some((x, y)) = common.iter().rev().find(|(x, y)| !x.is_ad() && !y.is_ad()) {
        let get = |u: String| async move { anyhow::Ok(tw.http().get(u).send().await?.error_for_status()?.bytes().await?) };
        let (bx, by) = tokio::try_join!(get(x.uri.clone()), get(y.uri.clone()))?;
        let packets = bx.len().min(by.len()) / 188;
        let equal = (0..packets).filter(|i| bx[i * 188..(i + 1) * 188] == by[i * 188..(i + 1) * 188]).count();
        println!(
            "newest common live segment (pdt {}): {} vs {} bytes, byte-identical: {}, equal TS packets: {equal}/{packets}",
            x.pdt.as_deref().unwrap_or("-"),
            bx.len(),
            by.len(),
            bx == by
        );
    }

    println!("CORS (request with Origin: http://localhost:8787):");
    let seg = Url::parse(&pa.segments.last().unwrap().uri)?;
    for (label, url) in [("segment (CDN)", seg), ("media playlist", a.uri.clone())] {
        let r = tw.http().get(url).header("Origin", "http://localhost:8787").send().await?;
        let acao = r.headers().get("access-control-allow-origin").map(|v| v.to_str().unwrap_or("?").to_string());
        println!("  {label:<16} status {}  access-control-allow-origin: {}", r.status(), acao.as_deref().unwrap_or("<none>"));
    }
    Ok(())
}
