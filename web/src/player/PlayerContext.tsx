// The player lives at the app root and survives navigation (D14). Pages only tell it what to
// play (a channel, or a multiview session); it shows full screen or as a mini player in the
// corner while you browse.

import { createContext, useCallback, useContext, useMemo, useState, type ReactNode } from "react";

export type PlayerMode = "full" | "mini";
export type Target = { kind: "channel"; channel: string } | { kind: "session"; id: string; name: string };

type PlayerApi = {
  target: Target | null;
  /** The channel being played in single view, if any. */
  channel: string | null;
  mode: PlayerMode;
  play: (channel: string) => void;
  playSession: (id: string, name: string) => void;
  minimize: () => void;
  expand: () => void;
  stop: () => void;
};

const PlayerCtx = createContext<PlayerApi | null>(null);

export function PlayerProvider({ children }: { children: ReactNode }) {
  const [target, setTarget] = useState<Target | null>(null);
  const [mode, setMode] = useState<PlayerMode>("full");

  const play = useCallback((c: string) => {
    setTarget({ kind: "channel", channel: c.toLowerCase() });
    setMode("full");
  }, []);
  const playSession = useCallback((id: string, name: string) => {
    setTarget({ kind: "session", id, name });
    setMode("full");
  }, []);
  const minimize = useCallback(() => setMode("mini"), []);
  const expand = useCallback(() => setMode("full"), []);
  const stop = useCallback(() => setTarget(null), []);

  const value = useMemo(
    () => ({
      target,
      channel: target?.kind === "channel" ? target.channel : null,
      mode,
      play,
      playSession,
      minimize,
      expand,
      stop,
    }),
    [target, mode, play, playSession, minimize, expand, stop],
  );
  return <PlayerCtx.Provider value={value}>{children}</PlayerCtx.Provider>;
}

export function usePlayer(): PlayerApi {
  const ctx = useContext(PlayerCtx);
  if (!ctx) throw new Error("usePlayer outside PlayerProvider");
  return ctx;
}
