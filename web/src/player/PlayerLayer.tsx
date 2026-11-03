// Renders the current target with hls.js: a single channel, or a multiview session (one video,
// one audio track per tile). Full screen with controls, or a mini player while browsing.
// Changing pages never unmounts it, so playback continues (D14).

import { useEffect, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import Hls, { type Level } from "hls.js";
import { api, ApiError, GRID, type MultiviewInfo, type PlayStatus, type SlotInfo } from "../api";
import { claimArrows, focusWithin } from "../tv";
import { ChatOverlay } from "./ChatOverlay";
import { QUALITY_KEY } from "../pages/Settings";
import { usePlayer, type Target } from "./PlayerContext";

type Quality = { index: number; label: string };

function levelLabel(l: Level): string {
  const fps = l.frameRate && l.frameRate > 30 ? Math.round(l.frameRate) : "";
  return l.height ? `${l.height}p${fps}` : "audio";
}

function targetKey(t: Target | null): string | null {
  return t ? (t.kind === "channel" ? `c:${t.channel}` : `s:${t.id}`) : null;
}

/** Shows that an ad is being skipped; for slates, how long until the stream is back. */
function AdBadge({ ad, hls }: { ad: PlayStatus["ad"]; hls: Hls | null }) {
  const [, tick] = useState(0);
  useEffect(() => {
    if (!ad) return;
    const t = setInterval(() => tick((n) => n + 1), 1000);
    return () => clearInterval(t);
  }, [ad]);
  if (!ad) return null;
  // The server reports stream time; the player's position in stream time is its program date
  // time. It runs behind the live edge, so only show the badge while it plays the ad's stretch.
  const playing = hls?.playingDate?.getTime();
  if (playing && (playing < ad.from || (ad.until && playing >= ad.until))) return null;
  if (ad.filled_by === "backup") return <div className="ad-badge">Ad skipped</div>;
  const left = ad.until && playing ? Math.max(0, Math.round((ad.until - playing) / 1000)) : null;
  return (
    <div className="ad-badge ad-badge--slate">
      Ad break{left !== null && left > 0 ? ` · back in ${Math.floor(left / 60)}:${String(left % 60).padStart(2, "0")}` : ""}
    </div>
  );
}

const STATE_TEXT: Record<SlotInfo["state"], string | null> = {
  live: null,
  loading: "Waiting for the stream…",
  ad: "Ad break, back in a moment",
  retrying: "Stream interrupted, reconnecting…",
  offline: "Offline",
};

/** Labels, states and the focus frame over the multiview video, positioned on the grid. */
function TileOverlay({ slots, focus, onFocus }: { slots: (SlotInfo | null)[]; focus: number; onFocus: (k: number) => void }) {
  const pct = (n: number) => `${(n / GRID) * 100}%`;
  return (
    <div className="tiles">
      {slots.map((s, k) =>
        s ? (
          <button
            key={k}
            className={`tile${k === focus ? " tile--focus" : ""}`}
            style={{ left: pct(s.x), top: pct(s.y), width: pct(s.w), height: pct(s.h), zIndex: s.z + 1 }}
            onClick={(e) => {
              e.stopPropagation();
              onFocus(k);
            }}
            title={`${s.channel}: click for its audio`}
          >
            <span className="tile__label">
              {s.channel}
              {k === focus && " 🔊"}
            </span>
            {STATE_TEXT[s.state] && <span className="tile__state">{STATE_TEXT[s.state]}</span>}
          </button>
        ) : null,
      )}
    </div>
  );
}

export function PlayerLayer() {
  const { target, mode, minimize, expand, stop } = usePlayer();
  const videoRef = useRef<HTMLVideoElement>(null);
  const hlsRef = useRef<Hls | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [qualities, setQualities] = useState<Quality[]>([]);
  const [quality, setQuality] = useState(-1); // -1 = auto
  const [muted, setMuted] = useState(false);
  const [playId, setPlayId] = useState<string | null>(null);
  const [focus, setFocus] = useState(0);
  // Bumped to start a broken multiview again (the server restarts a failed pipeline).
  const [attempt, setAttempt] = useState(0);
  const key = targetKey(target);
  const channel = target?.kind === "channel" ? target.channel : null;
  const sessionId = target?.kind === "session" ? target.id : null;

  const info = useQuery({
    queryKey: ["channel", channel],
    queryFn: () => api.channel(channel!),
    enabled: !!channel,
    refetchInterval: 60_000,
  });
  const status = useQuery({
    queryKey: ["play-status", playId],
    queryFn: () => api.playStatus(playId!),
    enabled: !!playId,
    refetchInterval: 2000,
  });
  const mv = useQuery<MultiviewInfo>({
    queryKey: ["multiview", sessionId],
    queryFn: () => api.multiviewStatus(sessionId!),
    enabled: !!sessionId && !loading,
    refetchInterval: 2000,
  });

  useEffect(() => {
    const video = videoRef.current;
    if (!target || !video) return;
    let cancelled = false;
    setError(null);
    setLoading(true);
    setQualities([]);
    setQuality(-1);
    setPlayId(null);
    if (attempt === 0) setFocus(0);

    const source: Promise<string> =
      target.kind === "channel"
        ? api.play(target.channel).then(({ id, hls_url }) => {
            if (!cancelled) setPlayId(id);
            return hls_url;
          })
        : api.playSession(target.id).then((m) => {
            const first = m.slots.findIndex((s) => s !== null);
            if (!cancelled && attempt === 0) setFocus(Math.max(0, first));
            return m.hls_url;
          });

    source
      .then((hls_url) => {
        if (cancelled) return;
        const start = () =>
          video.play().catch(() => {
            // Autoplay with sound can be blocked; start muted instead.
            video.muted = true;
            setMuted(true);
            return video.play();
          });
        if (Hls.isSupported()) {
          const hls = new Hls({ liveSyncDurationCount: 3, lowLatencyMode: false });
          hlsRef.current = hls;
          hls.on(Hls.Events.MANIFEST_PARSED, (_e, data) => {
            // This device's quality limit (Settings): cap automatic switching at that height.
            const limit = Number(localStorage.getItem(QUALITY_KEY));
            if (limit && target.kind === "channel") {
              const allowed = data.levels.map((l, i) => ({ h: l.height, i })).filter((l) => l.h > 0 && l.h <= limit);
              if (allowed.length) hls.autoLevelCapping = allowed.reduce((a, b) => (b.h > a.h ? b : a)).i;
            }
            setQualities(
              data.levels
                .map((l, index) => ({ index, label: levelLabel(l), height: l.height }))
                .filter((q) => q.height > 0)
                .map(({ index, label }) => ({ index, label })),
            );
            start();
          });
          hls.on(Hls.Events.ERROR, (_e, data) => {
            if (!data.fatal) return;
            hls.destroy();
            if (target.kind === "session" && attempt < 3) {
              setTimeout(() => !cancelled && setAttempt((a) => a + 1), 2000);
            } else {
              setError(`Playback failed (${data.details})`);
            }
          });
          hls.loadSource(hls_url);
          hls.attachMedia(video);
        } else {
          // Safari plays HLS natively.
          video.src = hls_url;
          start();
        }
      })
      .catch((e: unknown) => {
        if (!cancelled) setError(e instanceof ApiError ? e.message : "Could not start the stream");
      })
      .finally(() => !cancelled && setLoading(false));

    return () => {
      cancelled = true;
      hlsRef.current?.destroy();
      hlsRef.current = null;
      video.removeAttribute("src");
      video.load();
    };
    // `key` identifies the target; the object itself changes identity on every play().
  }, [key, attempt]);

  // A new target starts with a fresh retry budget.
  useEffect(() => setAttempt(0), [key]);

  useEffect(() => {
    if (hlsRef.current) hlsRef.current.currentLevel = quality;
  }, [quality]);

  useEffect(() => {
    if (videoRef.current) videoRef.current.muted = muted;
  }, [muted]);

  // Multiview: the focused tile's audio track plays (instant, no server round trip, D7).
  const audioTracks = hlsRef.current?.audioTracks.length ?? 0;
  useEffect(() => {
    const hls = hlsRef.current;
    if (!sessionId || !hls) return;
    const index = hls.audioTracks.findIndex((t) => t.name === `slot${focus}`);
    if (index >= 0 && hls.audioTrack !== index) hls.audioTrack = index;
  }, [focus, sessionId, audioTracks, loading]);

  // Multiview full screen: left/right switch the tile (unless the focus is in the chat or the
  // controls, which up/down reach through the normal navigation).
  useEffect(() => {
    if (sessionId && mode === "full")
      return claimArrows((e) => (e.key === "ArrowLeft" || e.key === "ArrowRight") && !focusWithin(".chat", ".player__bar"));
  }, [sessionId, mode]);

  // The remote's Play/Pause key (sent by the TV shell).
  useEffect(() => {
    const toggle = () => {
      const v = videoRef.current;
      if (v) void (v.paused ? v.play() : v.pause());
    };
    window.addEventListener("nitch:playpause", toggle);
    return () => window.removeEventListener("nitch:playpause", toggle);
  }, []);

  // The TV shell keeps the screen on while something plays.
  useEffect(() => {
    window.NitchTV?.setKeepScreenOn(!!target);
  }, [target]);

  const slots = mv.data?.slots ?? [];
  const occupied = slots.map((s, k) => (s ? k : -1)).filter((k) => k >= 0);

  // Escape leaves full screen view (back to browsing, stream keeps playing); arrows move the
  // multiview focus.
  useEffect(() => {
    if (mode !== "full" || !target) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") minimize();
      if (
        sessionId &&
        occupied.length > 0 &&
        (e.key === "ArrowRight" || e.key === "ArrowLeft") &&
        !focusWithin(".chat", ".player__bar")
      ) {
        const i = Math.max(0, occupied.indexOf(focus));
        const next = (i + (e.key === "ArrowRight" ? 1 : occupied.length - 1)) % occupied.length;
        setFocus(occupied[next]);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [mode, target, minimize, sessionId, occupied, focus]);

  if (!target) return <video ref={videoRef} hidden />;

  const live = info.data?.live;
  const title = target.kind === "channel" ? (info.data?.display_name ?? target.channel) : target.name;
  return (
    <div
      className={`player player--${mode}`}
      onClick={mode === "mini" ? expand : undefined}
      // The mini player is reachable with the remote: focus it, press OK to go back to full screen.
      tabIndex={mode === "mini" ? 0 : undefined}
      onKeyDown={mode === "mini" ? (e) => e.key === "Enter" && e.target === e.currentTarget && expand() : undefined}
    >
      <div className="player__stage">
        <video ref={videoRef} className="player__video" playsInline />
        {sessionId && mode === "full" && <TileOverlay slots={slots} focus={focus} onFocus={setFocus} />}
        {mode === "full" && (
          <ChatOverlay
            focused={channel ?? slots[focus]?.channel ?? null}
            all={channel ? [channel] : slots.flatMap((s) => (s ? [s.channel] : []))}
          />
        )}
      </div>
      {loading && <div className="player__status">{sessionId ? "Starting the multiview…" : "Loading…"}</div>}
      {error && <div className="player__status player__status--error">{error}</div>}
      {channel && <AdBadge ad={status.data?.ad ?? null} hls={hlsRef.current} />}
      {mode === "full" ? (
        <>
          <div className="player__bar player__bar--top">
            <button className="btn btn--ghost" onClick={minimize} title="Back (Esc)">
              ← Back
            </button>
            <div className="player__title">
              <strong>{title}</strong>
              {live && <span className="muted"> · {live.game}</span>}
              {live && <div className="player__subtitle">{live.title}</div>}
              {sessionId && slots[focus] && <div className="player__subtitle">Audio: {slots[focus]!.channel} (← → to switch)</div>}
            </div>
            <button className="btn btn--ghost" onClick={stop} title="Stop">
              ✕
            </button>
          </div>
          <div className="player__bar player__bar--bottom">
            <button className="btn btn--ghost" onClick={() => setMuted((m) => !m)}>
              {muted ? "Unmute" : "Mute"}
            </button>
            {channel && qualities.length > 0 && (
              <select className="select" value={quality} onChange={(e) => setQuality(Number(e.target.value))}>
                <option value={-1}>Auto</option>
                {qualities.map((q) => (
                  <option key={q.index} value={q.index}>
                    {q.label}
                  </option>
                ))}
              </select>
            )}
          </div>
        </>
      ) : (
        <div className="player__mini-bar">
          <span>{title}</span>
          <button
            className="btn btn--ghost btn--small"
            onClick={(e) => {
              e.stopPropagation();
              stop();
            }}
          >
            ✕
          </button>
        </div>
      )}
    </div>
  );
}
