//! Multiview pipelines run in their own process (`nitch-server multiview-worker`), so a crash in
//! native code (GStreamer, plugins) only takes down that pipeline, not the server. A segfault
//! when tiles' streams ended was reproduced in about 1 of 20 runs; the manager restarts a crashed
//! worker while someone watches the session.
//!
//! Protocol: JSON lines. The server writes commands to the worker's stdin; the worker writes its
//! status to stdout every second. Its logs go to stderr, which is inherited.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use nitch_media::{Config, Multiview, TileSpec};
use serde::{Deserialize, Serialize};

pub const SUBCOMMAND: &str = "multiview-worker";

#[derive(Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
enum Cmd {
    Start { config: Config },
    SetTile { slot: usize, spec: Option<TileSpec> },
    Stop,
}

#[derive(Serialize, Deserialize, Default, Clone)]
struct Status {
    states: Vec<Option<String>>,
    failure: Option<String>,
}

// ---------- worker process ----------

/// Entry point of the worker process: runs one pipeline until stdin closes or `stop` arrives.
pub fn run() -> Result<()> {
    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    let first = lines.next().context("no start command")??;
    let Cmd::Start { config } = serde_json::from_str(&first)? else {
        anyhow::bail!("first command must be start");
    };
    // `None` once stopped, so the status thread notices and the stop can take the pipeline.
    let dir = config.dir.display().to_string();
    let mv = Arc::new(Mutex::new(Some(Multiview::start(config)?)));
    tracing::info!(%dir, "multiview worker running");

    // Status reports, every second.
    let reporter = mv.clone();
    std::thread::spawn(move || {
        let mut out = std::io::stdout();
        loop {
            let status = {
                let mut guard = reporter.lock().unwrap();
                let Some(mv) = guard.as_mut() else { return };
                // Hidden rendition switches that are ready take over.
                if let Err(e) = mv.poll() {
                    tracing::warn!("switching a tile's rendition failed: {e:#}");
                }
                Status { states: mv.tile_states(), failure: mv.failure() }
            };
            let line = serde_json::to_string(&status).unwrap_or_default();
            if writeln!(out, "{line}").and_then(|_| out.flush()).is_err() {
                return;
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    });

    for line in lines {
        match serde_json::from_str::<Cmd>(&line?)? {
            Cmd::SetTile { slot, spec } => {
                if let Some(m) = mv.lock().unwrap().as_mut() {
                    if let Err(e) = m.set_tile(slot, spec.as_ref()) {
                        tracing::warn!(slot, "set_tile failed: {e:#}");
                    }
                }
            }
            Cmd::Stop => break,
            Cmd::Start { .. } => {}
        }
    }
    // stdin closed or stop: shut the pipeline down cleanly.
    if let Some(m) = mv.lock().unwrap().take() {
        m.stop();
    }
    Ok(())
}

// ---------- server side ----------

/// A running worker process; same surface as `Multiview` for the manager.
pub struct Worker {
    child: Child,
    stdin: ChildStdin,
    status: Arc<Mutex<Status>>,
    slots: usize,
}

impl Worker {
    pub fn start(config: Config) -> Result<Self> {
        let slots = config.slots;
        let mut child = Command::new(std::env::current_exe()?)
            .arg(SUBCOMMAND)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .context("starting the multiview worker")?;
        let mut stdin = child.stdin.take().context("worker stdin")?;
        let stdout = child.stdout.take().context("worker stdout")?;
        writeln!(stdin, "{}", serde_json::to_string(&Cmd::Start { config })?)?;
        stdin.flush()?;

        let status = Arc::new(Mutex::new(Status { states: vec![None; slots], failure: None }));
        let reader = status.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if let Ok(s) = serde_json::from_str::<Status>(&line) {
                    *reader.lock().unwrap() = s;
                }
            }
            // Output ended: the worker exited. Unless we stopped it, that's a crash.
            let mut s = reader.lock().unwrap();
            s.failure.get_or_insert_with(|| "multiview worker exited".into());
        });
        Ok(Self { child, stdin, status, slots })
    }

    fn send(&mut self, cmd: &Cmd) -> Result<()> {
        writeln!(self.stdin, "{}", serde_json::to_string(cmd)?)?;
        self.stdin.flush()?;
        Ok(())
    }

    pub fn set_tile(&mut self, slot: usize, spec: Option<&TileSpec>) -> Result<()> {
        anyhow::ensure!(slot < self.slots, "slot {slot} out of range");
        self.send(&Cmd::SetTile { slot, spec: spec.cloned() })
    }

    pub fn tile_states(&self) -> Vec<Option<String>> {
        self.status.lock().unwrap().states.clone()
    }

    pub fn failure(&self) -> Option<String> {
        self.status.lock().unwrap().failure.clone()
    }

    /// Asks the worker to stop; kills it if it doesn't exit within 5 s.
    pub fn stop(mut self) {
        let _ = self.send(&Cmd::Stop);
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Ok(Some(_)) = self.child.try_wait() {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        tracing::warn!("multiview worker didn't stop, killing it");
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        if let Ok(None) = self.child.try_wait() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}
