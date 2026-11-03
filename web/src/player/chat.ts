// Chat data: one WebSocket to the server for the whole app (D6, M6). The player says which
// channels are in view; their messages are kept here (the last KEEP per channel), whether or
// not the overlay shows them, so switching chat modes or turning the chat off and on keeps
// what was already there.

import { useSyncExternalStore } from "react";

export type ChatMessage = {
  channel: string;
  user: string;
  display_name: string;
  color: string;
  text: string;
  /** [emote id, first char, last char], counting Unicode code points */
  emotes: [string, number, number][];
  badges: string[];
  ts: number;
  /** Arrival order across all channels (set here). */
  seq: number;
};

/** Other server events on the same socket, e.g. `{type: "session_updated", id}`. */
export type ServerEvent = { type: string; id?: string };
const eventListeners = new Set<(e: ServerEvent) => void>();

const KEEP = 150;
let socket: WebSocket | null = null;
let wanted: string[] = [];
let retry = 1000;
let seq = 0;
/** channel -> its latest messages, oldest first */
const buffers = new Map<string, ChatMessage[]>();
const listeners = new Set<() => void>();
let version = 0;
let frame = 0;

function send() {
  if (socket?.readyState === WebSocket.OPEN) socket.send(JSON.stringify({ type: "chat_subscribe", channels: wanted }));
}

function connect() {
  const proto = location.protocol === "https:" ? "wss" : "ws";
  socket = new WebSocket(`${proto}://${location.host}/api/ws`);
  socket.onopen = () => {
    retry = 1000;
    send();
  };
  socket.onmessage = (e) => {
    const data = JSON.parse(e.data);
    if (data.type === "chat") receive(data as ChatMessage);
    else eventListeners.forEach((l) => l(data as ServerEvent));
  };
  socket.onclose = () => {
    socket = null;
    if (wanted.length > 0 || eventListeners.size > 0) setTimeout(connect, (retry = Math.min(retry * 2, 30_000)));
  };
}

function receive(m: ChatMessage) {
  const buf = buffers.get(m.channel);
  if (!buf) return;
  buf.push({ ...m, seq: seq++ });
  if (buf.length > KEEP) buf.splice(0, buf.length - KEEP);
  // One render per frame, however busy the chats are.
  if (!frame)
    frame = requestAnimationFrame(() => {
      frame = 0;
      version++;
      listeners.forEach((l) => l());
    });
}

/** The channels in view (all tiles); their chats are read and kept. Empty stops reading. */
export function setChatChannels(channels: string[]) {
  const next = [...new Set(channels.map((c) => c.toLowerCase()))].sort();
  if (next.join() === wanted.join()) return;
  for (const c of buffers.keys()) if (!next.includes(c)) buffers.delete(c);
  for (const c of next) if (!buffers.has(c)) buffers.set(c, []);
  wanted = next;
  version++;
  listeners.forEach((l) => l());
  if (!socket && wanted.length > 0) connect();
  else send();
}

const subscribe = (l: () => void) => {
  listeners.add(l);
  return () => listeners.delete(l);
};

let cacheKey = "";
let cache: ChatMessage[] = [];

/** Messages of `channels` in arrival order (the latest KEEP); re-renders as they come in. */
export function useChatMessages(channels: string[]): ChatMessage[] {
  const key = channels.join(",");
  return useSyncExternalStore(subscribe, () => {
    const k = `${version}|${key}`;
    if (k !== cacheKey) {
      cacheKey = k;
      cache = channels
        .flatMap((c) => buffers.get(c) ?? [])
        .sort((a, b) => a.seq - b.seq)
        .slice(-KEEP);
    }
    return cache;
  });
}

/** Splits a message into text and emote parts (emote positions count code points, not UTF-16). */
export function parts(m: ChatMessage): ({ text: string } | { emote: string; name: string })[] {
  if (m.emotes.length === 0) return [{ text: m.text }];
  const chars = Array.from(m.text);
  const sorted = [...m.emotes].sort((a, b) => a[1] - b[1]);
  const out: ({ text: string } | { emote: string; name: string })[] = [];
  let pos = 0;
  for (const [id, start, end] of sorted) {
    if (start < pos || end >= chars.length) continue;
    if (start > pos) out.push({ text: chars.slice(pos, start).join("") });
    out.push({ emote: id, name: chars.slice(start, end + 1).join("") });
    pos = end + 1;
  }
  if (pos < chars.length) out.push({ text: chars.slice(pos).join("") });
  return out;
}

/** Listens for server events (the socket stays open while anyone listens). */
export function watchEvents(onEvent: (e: ServerEvent) => void): () => void {
  eventListeners.add(onEvent);
  if (!socket) connect();
  return () => {
    eventListeners.delete(onEvent);
  };
}
