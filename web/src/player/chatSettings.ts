// Chat overlay settings, saved per device and shared by the overlay (mouse) and the side menu
// (remote). Position and size are percent of the screen.

import { useSyncExternalStore } from "react";

export type ChatMode = "off" | "focused" | "combined";
export type ChatSettings = {
  mode: ChatMode;
  x: number;
  y: number;
  w: number;
  h: number;
  /** Text size in rem. */
  font: number;
  /** Background opacity. */
  opacity: number;
  /** Text opacity. */
  text: number;
  /** Channels left out of the combined chat. */
  exclude: string[];
};

const KEY = "nitch.chat.v2";
export const CHAT_DEFAULTS: ChatSettings = { mode: "focused", x: 76, y: 4, w: 22, h: 70, font: 0.85, opacity: 0.45, text: 1, exclude: [] };
export const clamp = (v: number, lo: number, hi: number) => Math.max(lo, Math.min(hi, v));
const round = (v: number) => Math.round(v * 100) / 100;

function load(): ChatSettings {
  try {
    return { ...CHAT_DEFAULTS, ...JSON.parse(localStorage.getItem(KEY) ?? "{}") };
  } catch {
    return CHAT_DEFAULTS;
  }
}

let current = load();
const listeners = new Set<() => void>();

/** Applies `patch`, keeping the box on screen and the values in range. */
export function updateChat(patch: Partial<ChatSettings> | ((s: ChatSettings) => Partial<ChatSettings>)) {
  const p = typeof patch === "function" ? patch(current) : patch;
  const n = { ...current, ...p };
  n.w = clamp(n.w, 10, 100);
  n.h = clamp(n.h, 12, 100);
  n.x = clamp(n.x, 0, 100 - n.w);
  n.y = clamp(n.y, 0, 100 - n.h);
  n.font = round(clamp(n.font, 0.5, 2.5));
  n.opacity = round(clamp(n.opacity, 0, 0.95));
  n.text = round(clamp(n.text, 0.3, 1));
  current = n;
  localStorage.setItem(KEY, JSON.stringify(n));
  listeners.forEach((l) => l());
}

export function resetChat() {
  updateChat(CHAT_DEFAULTS);
}

export function useChatSettings(): ChatSettings {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    () => current,
  );
}

/** Quick positions for the remote (and the mouse): a column at either side, or a strip at the bottom. */
export const CHAT_PLACES: { label: string; rect: Pick<ChatSettings, "x" | "y" | "w" | "h"> }[] = [
  { label: "Right", rect: { x: 76, y: 4, w: 22, h: 70 } },
  { label: "Left", rect: { x: 2, y: 4, w: 22, h: 70 } },
  { label: "Right, full height", rect: { x: 78, y: 0, w: 22, h: 100 } },
  { label: "Bottom left", rect: { x: 2, y: 60, w: 30, h: 36 } },
];

/** A stable color per channel (stripe and label in the combined chat). */
export function channelColor(channel: string): string {
  let h = 0;
  for (const c of channel) h = (h * 31 + c.charCodeAt(0)) % 360;
  return `hsl(${h} 70% 62%)`;
}
