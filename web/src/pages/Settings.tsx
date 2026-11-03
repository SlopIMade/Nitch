// Settings (M7): per-device preferences, the TV app's server, and diagnostics from the server.
// Like every page, this opens while the player keeps running as the mini player (D14).

import { useState } from "react";
import { useQuery } from "@tanstack/react-query";

export const QUALITY_KEY = "nitch.quality";

type System = {
  version: string;
  uptime_seconds: number;
  multiview: {
    encoder: string;
    max_running: number;
    running: { id: string; idle_seconds: number | null; slots: { channel: string; state: string }[] }[];
  };
  play_sessions: { id: string; channel: string; idle_seconds: number; slates: boolean; ad: unknown }[];
  chat_channels: Record<string, number>;
};

function duration(s: number): string {
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  return h > 0 ? `${h} h ${m} min` : `${m} min`;
}

export function SettingsPage() {
  const [quality, setQuality] = useState(() => localStorage.getItem(QUALITY_KEY) ?? "auto");
  const [chatReset, setChatReset] = useState(false);
  const system = useQuery<System>({
    queryKey: ["system"],
    queryFn: () => fetch("/api/system").then((r) => r.json()),
    refetchInterval: 5000,
  });

  const saveQuality = (q: string) => {
    setQuality(q);
    if (q === "auto") localStorage.removeItem(QUALITY_KEY);
    else localStorage.setItem(QUALITY_KEY, q);
  };

  const sys = system.data;
  return (
    <div className="page settings">
      <h2>Settings</h2>

      <section className="settings__section">
        <h3>Playback (this device)</h3>
        <label className="settings__row">
          <span>Highest quality for single streams</span>
          <select className="select" value={quality} onChange={(e) => saveQuality(e.target.value)}>
            <option value="auto">Auto (best the connection allows)</option>
            <option value="1080">1080p</option>
            <option value="720">720p</option>
            <option value="480">480p</option>
            <option value="360">360p</option>
          </select>
        </label>
        <p className="muted">Applies the next time a stream starts.</p>
      </section>

      <section className="settings__section">
        <h3>Chat (this device)</h3>
        <div className="settings__row">
          <span>Position, size, font and opacity of the chat overlay</span>
          <button
            className="btn btn--ghost"
            onClick={() => {
              localStorage.removeItem("nitch.chat");
              setChatReset(true);
            }}
          >
            {chatReset ? "Reset ✓" : "Reset to default"}
          </button>
        </div>
      </section>

      {window.NitchTV && (
        <section className="settings__section">
          <h3>TV app</h3>
          <div className="settings__row">
            <span>Server: {location.host}</span>
            <button className="btn btn--ghost" onClick={() => window.NitchTV?.changeServer?.()}>
              Change server
            </button>
          </div>
        </section>
      )}

      <section className="settings__section">
        <h3>Server</h3>
        {system.isError && <p className="error">Could not load the server status.</p>}
        {sys && (
          <dl className="settings__facts">
            <dt>Version</dt>
            <dd>
              {sys.version}, running for {duration(sys.uptime_seconds)}
            </dd>
            <dt>Multiview encoder</dt>
            <dd>
              {sys.multiview.encoder} (at most {sys.multiview.max_running} running at once)
            </dd>
            <dt>Running multiviews</dt>
            <dd>
              {sys.multiview.running.length === 0
                ? "none"
                : sys.multiview.running.map((r) => (
                    <div key={r.id}>
                      {r.slots.map((s) => `${s.channel} (${s.state})`).join(", ")}
                    </div>
                  ))}
            </dd>
            <dt>Streams being read</dt>
            <dd>{sys.play_sessions.length === 0 ? "none" : sys.play_sessions.map((p) => p.channel).join(", ")}</dd>
            <dt>Chats joined</dt>
            <dd>{Object.keys(sys.chat_channels).length === 0 ? "none" : Object.keys(sys.chat_channels).join(", ")}</dd>
          </dl>
        )}
      </section>
    </div>
  );
}
