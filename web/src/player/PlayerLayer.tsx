// Renders the current stream with hls.js. Full screen with controls, or a mini player while
// browsing. Changing pages never unmounts it, so playback continues (D14).

import { useEffect, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import Hls, { type Level } from "hls.js";
import { api, ApiError } from "../api";
import { usePlayer } from "./PlayerContext";

type Quality = { index: number; label: string };

function levelLabel(l: Level): string {
  const fps = l.frameRate && l.frameRate > 30 ? Math.round(l.frameRate) : "";
  return l.height ? `${l.height}p${fps}` : "audio";
}

export function PlayerLayer() {
  const { channel, mode, minimize, expand, stop } = usePlayer();
  const videoRef = useRef<HTMLVideoElement>(null);
  const hlsRef = useRef<Hls | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [qualities, setQualities] = useState<Quality[]>([]);
  const [quality, setQuality] = useState(-1); // -1 = auto
  const [muted, setMuted] = useState(false);

  const info = useQuery({
    queryKey: ["channel", channel],
    queryFn: () => api.channel(channel!),
    enabled: !!channel,
    refetchInterval: 60_000,
  });

  useEffect(() => {
    const video = videoRef.current;
    if (!channel || !video) return;
    let cancelled = false;
    setError(null);
    setLoading(true);
    setQualities([]);
    setQuality(-1);

    api
      .play(channel)
      .then(({ hls_url }) => {
        if (cancelled) return;
        const start = () => video.play().catch(() => {
          // Autoplay with sound can be blocked; start muted instead.
          video.muted = true;
          setMuted(true);
          return video.play();
        });
        if (Hls.isSupported()) {
          const hls = new Hls({ liveSyncDurationCount: 3, lowLatencyMode: false });
          hlsRef.current = hls;
          hls.on(Hls.Events.MANIFEST_PARSED, (_e, data) => {
            setQualities(
              data.levels
                .map((l, index) => ({ index, label: levelLabel(l), height: l.height }))
                .filter((q) => q.height > 0)
                .map(({ index, label }) => ({ index, label })),
            );
            start();
          });
          hls.on(Hls.Events.ERROR, (_e, data) => {
            if (data.fatal) {
              setError(`Playback failed (${data.details})`);
              hls.destroy();
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
  }, [channel]);

  useEffect(() => {
    if (hlsRef.current) hlsRef.current.currentLevel = quality;
  }, [quality]);

  useEffect(() => {
    if (videoRef.current) videoRef.current.muted = muted;
  }, [muted]);

  // Escape leaves full screen view (back to browsing, stream keeps playing).
  useEffect(() => {
    if (mode !== "full" || !channel) return;
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && minimize();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [mode, channel, minimize]);

  if (!channel) return <video ref={videoRef} hidden />;

  const live = info.data?.live;
  return (
    <div className={`player player--${mode}`} onClick={mode === "mini" ? expand : undefined}>
      <video ref={videoRef} className="player__video" playsInline />
      {loading && <div className="player__status">Loading…</div>}
      {error && <div className="player__status player__status--error">{error}</div>}
      {mode === "full" ? (
        <>
          <div className="player__bar player__bar--top">
            <button className="btn btn--ghost" onClick={minimize} title="Back (Esc)">
              ← Back
            </button>
            <div className="player__title">
              <strong>{info.data?.display_name ?? channel}</strong>
              {live && <span className="muted"> · {live.game}</span>}
              {live && <div className="player__subtitle">{live.title}</div>}
            </div>
            <button className="btn btn--ghost" onClick={stop} title="Stop">
              ✕
            </button>
          </div>
          <div className="player__bar player__bar--bottom">
            <button className="btn btn--ghost" onClick={() => setMuted((m) => !m)}>
              {muted ? "Unmute" : "Mute"}
            </button>
            {qualities.length > 0 && (
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
          <span>{info.data?.display_name ?? channel}</span>
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
