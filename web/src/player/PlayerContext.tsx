// The player lives at the app root and survives navigation (D14). Pages only tell it what to
// play; it shows full screen or as a mini player in the corner while you browse.

import { createContext, useCallback, useContext, useMemo, useState, type ReactNode } from "react";

export type PlayerMode = "full" | "mini";

type PlayerApi = {
  channel: string | null;
  mode: PlayerMode;
  play: (channel: string) => void;
  minimize: () => void;
  expand: () => void;
  stop: () => void;
};

const PlayerCtx = createContext<PlayerApi | null>(null);

export function PlayerProvider({ children }: { children: ReactNode }) {
  const [channel, setChannel] = useState<string | null>(null);
  const [mode, setMode] = useState<PlayerMode>("full");

  const play = useCallback((c: string) => {
    setChannel(c.toLowerCase());
    setMode("full");
  }, []);
  const minimize = useCallback(() => setMode("mini"), []);
  const expand = useCallback(() => setMode("full"), []);
  const stop = useCallback(() => setChannel(null), []);

  const value = useMemo(
    () => ({ channel, mode, play, minimize, expand, stop }),
    [channel, mode, play, minimize, expand, stop],
  );
  return <PlayerCtx.Provider value={value}>{children}</PlayerCtx.Provider>;
}

export function usePlayer(): PlayerApi {
  const ctx = useContext(PlayerCtx);
  if (!ctx) throw new Error("usePlayer outside PlayerProvider");
  return ctx;
}
