// The player: a single channel, or a multiview session (one video, one audio track per tile),
// always full screen. Changing pages never unmounts it (D14); Back or ✕ ends playback.
//
// Switching what plays is seamless: the old stream keeps playing until the new one is ready,
// so adding a second stream to a single view (which turns it into a multiview session) or
// picking another channel never leaves the screen silent.
//
// Remote: ← → switch the tile (and its audio), hold ← for the side menu (add streams, chat),
// ↑ ↓ reach the controls, OK shows them, Back leaves.

import { useCallback, useEffect, useRef, useState } from "react";
import { useQueries, useQuery, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "react-router";
import Hls, { type Level } from "hls.js";
import { api, ApiError, GRID, type MultiviewInfo, type PlayStatus, type SlotInfo } from "../api";
import { addTile, removeTile } from "../layout";
import { claimArrows, focusWithin, pressKind } from "../tv";
import { setChatChannels } from "./chat";
import { ChatOverlay, type ChatAdjust } from "./ChatOverlay";
import { QUALITY_KEY } from "../pages/Settings";
import { usePlayer, type Target } from "./PlayerContext";
import { SideMenu, type MenuTab } from "./SideMenu";

const MAX_TILES = 9;

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
  retrying: "Reconnecting…",
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
            tabIndex={-1}
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

/**
 * The tile layout as the viewer sees it: the server changes the picture immediately, but the
 * player shows it only after its distance to the live edge, so overlays follow that late.
 */
function useShownSlots(slots: (SlotInfo | null)[] | undefined, hls: Hls | null): (SlotInfo | null)[] {
  const [shown, setShown] = useState<(SlotInfo | null)[]>([]);
  const layout = JSON.stringify((slots ?? []).map((s) => s && [s.channel, s.x, s.y, s.w, s.h, s.z]));
  const latest = useRef(slots);
  latest.current = slots;
  const first = shown.length === 0;
  useEffect(() => {
    if (!latest.current) {
      setShown([]);
      return;
    }
    if (first) {
      setShown(latest.current);
      return;
    }
    const delay = ((hls?.latency ?? 6) + 1) * 1000;
    const t = setTimeout(() => latest.current && setShown(latest.current), delay);
    return () => clearTimeout(t);
    // Only layout changes are delayed; states (live, ad, …) are taken along when they land.
  }, [layout]);
  // Tile states (live, ad, …) update right away.
  return shown.map((s) => s && { ...s, state: slots?.find((n) => n?.channel === s.channel)?.state ?? s.state });
}

export function PlayerLayer() {
  const { target, play, playSession, stop } = usePlayer();
  const navigate = useNavigate();
  const qc = useQueryClient();
  const videoRef = useRef<HTMLVideoElement>(null);
  const hlsRef = useRef<Hls | null>(null);
  /** What's attached to the video right now (it lags `target` while a switch is starting). */
  const active = useRef<{ key: string; playId: string | null } | null>(null);
  const [activeKey, setActiveKey] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [qualities, setQualities] = useState<Quality[]>([]);
  const [quality, setQuality] = useState(-1); // -1 = auto
  const [muted, setMuted] = useState(false);
  const [playId, setPlayId] = useState<string | null>(null);
  const [focus, setFocus] = useState(0);
  const [audioTracks, setAudioTracks] = useState(0);
  const [menu, setMenu] = useState<MenuTab | null>(null);
  const [chatAdjust, setChatAdjust] = useState<ChatAdjust>(null);
  const [controls, setControls] = useState(true);
  const [busy, setBusy] = useState<string | null>(null);
  // Bumped to start a broken multiview again (the server restarts a failed pipeline).
  const [attempt, setAttempt] = useState(0);
  const key = targetKey(target);
  const ready = activeKey === key;
  const channel = target?.kind === "channel" ? target.channel : null;
  const sessionId = target?.kind === "session" ? target.id : null;

  const status = useQuery({
    queryKey: ["play-status", playId],
    queryFn: () => api.playStatus(playId!),
    enabled: !!playId && !!channel,
    refetchInterval: 2000,
  });
  const session = useQuery({ queryKey: ["session", sessionId], queryFn: () => api.session(sessionId!), enabled: !!sessionId });
  const mv = useQuery<MultiviewInfo>({
    queryKey: ["multiview", sessionId],
    queryFn: () => api.multiviewStatus(sessionId!),
    enabled: !!sessionId && ready,
    refetchInterval: 2000,
  });
  const slots = useShownSlots(ready ? mv.data?.slots : undefined, hlsRef.current);
  const occupied = slots.map((s, k) => (s ? k : -1)).filter((k) => k >= 0);

  // The channels in view, and their stream info (titles: recommendations in the side menu).
  const inView = channel ? [channel] : (session.data?.tiles.map((t) => t.channel) ?? []);
  const infos = useQueries({
    queries: inView.map((login) => ({ queryKey: ["channel", login], queryFn: () => api.channel(login), refetchInterval: 60_000 })),
  });
  const titles = infos.flatMap((q) => (q.data?.live ? [q.data.live.title] : []));
  const focusedChannel = channel ?? slots[focus]?.channel ?? null;

  // Chat of everything in view is read (and kept) while the player is open.
  const chatKey = inView.join(",");
  useEffect(() => {
    setChatChannels(chatKey ? chatKey.split(",") : []);
  }, [chatKey]);
  useEffect(() => () => setChatChannels([]), []);

  // ---------- playback ----------

  useEffect(() => {
    const video = videoRef.current;
    if (!target || !video || !key) return;
    let cancelled = false;
    setError(null);

    const source: Promise<{ url: string; playId: string | null; first: number }> =
      target.kind === "channel"
        ? api.play(target.channel).then(({ id, hls_url }) => ({ url: hls_url, playId: id, first: 0 }))
        : api.playSession(target.id).then((m) => ({ url: m.hls_url, playId: null, first: Math.max(0, m.slots.findIndex((s) => s !== null)) }));

    source
      .then(({ url, playId: newPlayId, first }) => {
        if (cancelled) {
          if (newPlayId) void api.stopPlay(newPlayId);
          return;
        }
        // The new stream is ready: replace the old one now.
        hlsRef.current?.destroy();
        hlsRef.current = null;
        if (active.current?.playId && active.current.playId !== newPlayId) void api.stopPlay(active.current.playId);
        active.current = { key, playId: newPlayId };
        setActiveKey(key);
        setPlayId(newPlayId);
        setQualities([]);
        setQuality(-1);
        setAudioTracks(0);
        if (attempt === 0) setFocus(first);

        const start = () =>
          video.play().catch(() => {
            // Autoplay with sound can be blocked; start muted instead.
            video.muted = true;
            setMuted(true);
            return video.play();
          });
        if (!Hls.isSupported()) {
          // Safari plays HLS natively.
          video.src = url;
          void start();
          return;
        }
        const hls = new Hls({
          liveSyncDurationCount: 3,
          lowLatencyMode: false,
          // Never drift away from live: after a stall, play up to 8 % faster until back at
          // the target distance; beyond 8 segments, jump.
          maxLiveSyncPlaybackRate: 1.08,
          liveMaxLatencyDurationCount: 8,
        });
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
          void start();
        });
        hls.on(Hls.Events.AUDIO_TRACKS_UPDATED, (_e, data) => setAudioTracks(data.audioTracks.length));
        hls.on(Hls.Events.ERROR, (_e, data) => {
          if (!data.fatal) return;
          hls.destroy();
          if (hlsRef.current === hls) hlsRef.current = null;
          if (target.kind === "session" && attempt < 3) {
            setTimeout(() => setAttempt((a) => a + 1), 2000);
          } else {
            setError(`Playback failed (${data.details})`);
          }
        });
        hls.loadSource(url);
        hls.attachMedia(video);
      })
      .catch((e: unknown) => {
        if (!cancelled) setError(e instanceof ApiError ? e.message : "Could not start the stream");
      });

    return () => {
      cancelled = true;
    };
    // `key` identifies the target; the object itself changes identity on every play().
  }, [key, attempt]);

  // A new target starts with a fresh retry budget.
  useEffect(() => setAttempt(0), [key]);

  // Nothing to play: tear everything down (and tell the server the stream isn't watched).
  const teardown = useCallback(() => {
    hlsRef.current?.destroy();
    hlsRef.current = null;
    if (active.current?.playId) void api.stopPlay(active.current.playId);
    active.current = null;
    setActiveKey(null);
    setPlayId(null);
    setMenu(null);
    setChatAdjust(null);
  }, []);
  useEffect(() => {
    if (!target) teardown();
  }, [target, teardown]);
  useEffect(() => teardown, [teardown]);

  useEffect(() => {
    if (hlsRef.current) hlsRef.current.currentLevel = quality;
  }, [quality]);

  useEffect(() => {
    if (videoRef.current) videoRef.current.muted = muted;
  }, [muted]);

  // Multiview: the focused tile's audio track plays (instant, no server round trip, D7).
  useEffect(() => {
    const hls = hlsRef.current;
    if (!sessionId || !hls || !ready) return;
    const index = hls.audioTracks.findIndex((t) => t.name === `slot${focus}`);
    if (index >= 0 && hls.audioTrack !== index) hls.audioTrack = index;
  }, [focus, sessionId, audioTracks, ready]);

  // A focused tile that went away: focus the first one left.
  useEffect(() => {
    if (sessionId && occupied.length > 0 && !occupied.includes(focus)) setFocus(occupied[0]);
  }, [sessionId, occupied.join(), focus]);

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

  // ---------- controls ----------

  const hideTimer = useRef(0);
  const poke = useCallback(() => {
    setControls(true);
    clearTimeout(hideTimer.current);
    hideTimer.current = window.setTimeout(() => setControls(false), 3500);
  }, []);
  useEffect(() => {
    if (target) poke();
  }, [key, poke]);

  const openMenu = (tab: MenuTab) => {
    setChatAdjust(null);
    setMenu(tab);
  };
  const closeMenu = useCallback(() => {
    setMenu(null);
    (document.activeElement as HTMLElement | null)?.blur();
  }, []);
  const adjustDone = useCallback(() => setChatAdjust(null), []);

  // ← → switch the tile, holding ← opens the menu. Only while the focus isn't in the controls.
  const playerKeys = !!target && !menu && !chatAdjust;
  useEffect(() => {
    if (!playerKeys) return;
    const inControls = () => focusWithin(".player__bar");
    const release = claimArrows((e) => (e.key === "ArrowLeft" || e.key === "ArrowRight") && !inControls());
    const step = (d: number) => {
      if (!sessionId || occupied.length < 2) return;
      const i = Math.max(0, occupied.indexOf(focus));
      setFocus(occupied[(i + d + occupied.length) % occupied.length]);
    };
    const onDown = (e: KeyboardEvent) => {
      poke();
      if (e.key === "Escape") {
        stop();
        return;
      }
      if (inControls()) return;
      if (e.key === "ArrowLeft" && pressKind(e, "ArrowLeft") === "long") openMenu("add");
      if (e.key === "ArrowRight" && !e.repeat) {
        e.preventDefault();
        step(1);
      }
      if (e.key === "Enter" && (document.activeElement === document.body || !document.activeElement)) {
        e.preventDefault();
        document.querySelector<HTMLElement>(".player__bar--bottom button")?.focus();
      }
    };
    const onUp = (e: KeyboardEvent) => {
      if (!inControls() && pressKind(e, "ArrowLeft") === "short") step(-1);
    };
    window.addEventListener("keydown", onDown);
    window.addEventListener("keyup", onUp);
    return () => {
      release();
      window.removeEventListener("keydown", onDown);
      window.removeEventListener("keyup", onUp);
    };
  }, [playerKeys, sessionId, occupied.join(), focus, stop, poke]);

  // ---------- changing what's in view ----------

  const tiles = session.data?.tiles ?? [];
  const full = !!sessionId && tiles.length >= MAX_TILES;

  const add = async (login: string) => {
    setMenu(null);
    try {
      if (channel) {
        if (login === channel) return;
        setBusy(`Adding ${login}…`);
        const s = await api.createSession(`${channel} + ${login}`, addTile(addTile([], channel), login));
        qc.invalidateQueries({ queryKey: ["sessions"] });
        playSession(s.id, s.name);
      } else if (sessionId && session.data) {
        if (tiles.some((t) => t.channel === login) || full) return;
        setBusy(`Adding ${login}…`);
        const s = await api.updateSession(sessionId, { tiles: addTile(tiles, login) });
        qc.setQueryData(["session", sessionId], s);
        qc.invalidateQueries({ queryKey: ["sessions"] });
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : "Could not add the stream");
    } finally {
      setBusy(null);
    }
  };

  const remove = async (c: string) => {
    if (!sessionId || tiles.length < 2) return;
    const s = await api.updateSession(sessionId, { tiles: removeTile(tiles, tiles.findIndex((t) => t.channel === c)) });
    qc.setQueryData(["session", sessionId], s);
    qc.invalidateQueries({ queryKey: ["sessions"] });
  };

  const focusTile = (c: string) => {
    const k = slots.findIndex((s) => s?.channel === c);
    if (k >= 0) setFocus(k);
    closeMenu();
  };

  // While a switch is starting, the old stream plays on; say what's happening.
  const starting = !!target && !ready && !error;
  useEffect(() => {
    if (!starting) return;
    setBusy(sessionId ? "Starting the multiview…" : "Loading…");
    return () => setBusy(null);
  }, [starting, sessionId]);

  if (!target) return null;

  const info = infos[0]?.data;
  const live = channel ? info?.live : null;
  const title = target.kind === "channel" ? (info?.display_name ?? target.channel) : target.name;
  return (
    <div
      className={`player${controls || menu ? " player--controls" : ""}`}
      onMouseMove={poke}
      onClick={() => menu && closeMenu()}
    >
      <div className="player__stage">
        <video ref={videoRef} className="player__video" playsInline />
        {sessionId && ready && <TileOverlay slots={slots} focus={focus} onFocus={setFocus} />}
      </div>
      {activeKey && (
        <ChatOverlay
          focused={focusedChannel}
          all={inView}
          adjust={chatAdjust}
          onAdjustDone={adjustDone}
          onSettings={() => openMenu("chat")}
        />
      )}
      {!activeKey && !error && <div className="player__status">{sessionId ? "Starting the multiview…" : "Loading…"}</div>}
      {activeKey && busy && <div className="player__toast">{busy}</div>}
      {error && <div className="player__status player__status--error">{error}</div>}
      {channel && ready && <AdBadge ad={status.data?.ad ?? null} hls={hlsRef.current} />}

      {/* The bottom bar comes first: with nothing focused, the remote's arrows start there. */}
      <div className="player__bar player__bar--bottom">
        <button className="btn btn--ghost" onClick={() => openMenu("add")} title="Add streams (hold ← on the remote)">
          ＋ Streams
        </button>
        <button className="btn btn--ghost" onClick={() => openMenu("chat")}>
          Chat
        </button>
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
        <span className="player__keys">{sessionId ? "← → tile · hold ← menu" : "hold ← menu"}</span>
      </div>

      <div className="player__bar player__bar--top">
        <button className="btn btn--ghost" onClick={stop} title="Back (Esc)">
          ← Back
        </button>
        <div className="player__title">
          <strong>{title}</strong>
          {live && <span className="muted"> · {live.game}</span>}
          {live && <div className="player__subtitle">{live.title}</div>}
          {sessionId && slots[focus] && <div className="player__subtitle">Audio: {slots[focus]!.channel}</div>}
        </div>
      </div>
      {menu && (
        <SideMenu
          tab={menu}
          onTab={setMenu}
          onClose={closeMenu}
          inView={inView}
          titles={titles}
          onWatch={(login) => {
            closeMenu();
            if (login !== channel) play(login);
          }}
          onAdd={add}
          onRemove={sessionId && tiles.length > 1 ? remove : null}
          onFocusTile={focusTile}
          onEditLayout={
            sessionId
              ? () => {
                  stop();
                  navigate(`/multiview/${sessionId}`);
                }
              : null
          }
          onAdjustChat={(a) => {
            setMenu(null);
            setChatAdjust(a);
          }}
          full={full}
        />
      )}
    </div>
  );
}
