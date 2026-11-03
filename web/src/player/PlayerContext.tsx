// The player lives at the app root and survives navigation (D14). Pages only tell it what to
// play (a channel, or a multiview session); it covers the whole screen until it's stopped.

import { createContext, useCallback, useContext, useMemo, useState, type ReactNode } from "react";

export type Target = { kind: "channel"; channel: string } | { kind: "session"; id: string; name: string };

type PlayerApi = {
  target: Target | null;
  play: (channel: string) => void;
  playSession: (id: string, name: string) => void;
  stop: () => void;
};

const PlayerCtx = createContext<PlayerApi | null>(null);

export function PlayerProvider({ children }: { children: ReactNode }) {
  const [target, setTarget] = useState<Target | null>(null);
  const play = useCallback((c: string) => setTarget({ kind: "channel", channel: c.toLowerCase() }), []);
  const playSession = useCallback((id: string, name: string) => setTarget({ kind: "session", id, name }), []);
  const stop = useCallback(() => setTarget(null), []);
  const value = useMemo(() => ({ target, play, playSession, stop }), [target, play, playSession, stop]);
  return <PlayerCtx.Provider value={value}>{children}</PlayerCtx.Provider>;
}

export function usePlayer(): PlayerApi {
  const ctx = useContext(PlayerCtx);
  if (!ctx) throw new Error("usePlayer outside PlayerProvider");
  return ctx;
}
