// Chat overlay settings, saved per device and shared by the overlay (mouse) and the side menu
// (remote). Position and size are percent of the screen.

import { useSyncExternalStore } from "react";

export type ChatMode = "off" | "focused" | "combined";
export type ChatSettings = {
  mode: ChatMode;
  /** Over the picture: anywhere on the screen. */
  x: number;
  y: number;
  w: number;
  h: number;
  /** Beside the picture: a column at the right edge, the picture shrinks next to it. */
  dock: boolean;
  /** Its width, percent of the screen. */
  dockW: number;
  /** Text size in rem. */
  font: number;
  /** Background opacity. */
  opacity: number;
  /** Text opacity. */
  text: number;
  /** Channels left out of the combined chat. */
  exclude: string[];
  /** 7TV emotes (D37). */
  thirdPartyEmotes: boolean;
  /** Timeouts and bans as lines, and hidden messages as "message deleted" (D37). */
  moderation: boolean;
};

const KEY = "nitch.chat.v2";
export const CHAT_DEFAULTS: ChatSettings = {
  mode: "focused",
  x: 76,
  y: 4,
  w: 22,
  h: 70,
  dock: false,
  dockW: 24,
  font: 0.85,
  opacity: 0.45,
  text: 1,
  exclude: [],
  thirdPartyEmotes: true,
  moderation: true,
};
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
  n.dockW = clamp(n.dockW, 12, 50);
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

/** Back to the default place and size (the look and the mode stay). */
export function resetChatPlace() {
  const { x, y, w, h, dockW } = CHAT_DEFAULTS;
  updateChat({ x, y, w, h, dockW });
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

export type ChatRect = Pick<ChatSettings, "x" | "y" | "w" | "h">;

/** Quick places over the picture: the four corners and a full-height column at either side. */
export const CHAT_PLACES = {
  topLeft: { label: "Top left", rect: { x: 2, y: 4, w: 22, h: 45 } },
  topRight: { label: "Top right", rect: { x: 76, y: 4, w: 22, h: 45 } },
  bottomRight: { label: "Bottom right", rect: { x: 76, y: 51, w: 22, h: 45 } },
  bottomLeft: { label: "Bottom left", rect: { x: 2, y: 51, w: 22, h: 45 } },
  left: { label: "Left, full height", rect: { x: 0, y: 0, w: 22, h: 100 } },
  right: { label: "Right, full height", rect: { x: 78, y: 0, w: 22, h: 100 } },
} satisfies Record<string, { label: string; rect: ChatRect }>;

export type ChatPlace = keyof typeof CHAT_PLACES;

/** The place the chat is at, if it's exactly one of the quick places. */
export function chatPlace(s: ChatSettings): ChatPlace | null {
  for (const [k, p] of Object.entries(CHAT_PLACES)) {
    if (p.rect.x === s.x && p.rect.y === s.y && p.rect.w === s.w && p.rect.h === s.h) return k as ChatPlace;
  }
  return null;
}

/** A stable color per channel (stripe and label in the combined chat). */
export function channelColor(channel: string): string {
  let h = 0;
  for (const c of channel) h = (h * 31 + c.charCodeAt(0)) % 360;
  return `hsl(${h} 70% 62%)`;
}
