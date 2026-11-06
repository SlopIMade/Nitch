// Chat data: one WebSocket to the server for the whole app (D6, M6). The player says which
// channels are in view; their messages are kept here (the last KEEP per channel), whether or
// not the overlay shows them, so switching chat modes or turning the chat off and on keeps
// what was already there. On subscribing, the server sends what it already has of a channel
// (it reads the followed live channels in the background), so a chat opens with history.
// Moderation events arrive as lines too (D37): a timeout or ban hides that person's messages,
// a deletion hides one message, a clear hides everything before it. 7TV emotes are looked up
// per channel and replace matching words.

import { useSyncExternalStore } from "react";

export type Notice = { kind: "timeout"; seconds: number } | { kind: "ban" } | { kind: "clear" } | { kind: "delete"; target: string };

export type ChatMessage = {
  /** Twitch's message id (empty for notices). */
  id?: string;
  channel: string;
  user: string;
  display_name: string;
  color: string;
  text: string;
  /** [emote id, first char, last char], counting Unicode code points */
  emotes: [string, number, number][];
  badges: string[];
  ts: number;
  /** A moderation event instead of a message; `user` is who it's about. */
  notice?: Notice;
  /** Hidden by a moderator (a later notice). */
  deleted?: boolean;
  /** Arrival order across all channels (set here; also the React key). */
  seq: number;
};

/** Per channel: 7TV emote name → id. */
const emoteSets = new Map<string, Record<string, string>>();

function loadEmotes(channel: string) {
  fetch(`/api/emotes/${encodeURIComponent(channel)}`)
    .then((r) => (r.ok ? r.json() : null))
    .then((data: { emotes: Record<string, string> } | null) => {
      if (!data || !buffers.has(channel)) return;
      emoteSets.set(channel, data.emotes);
      version++;
      listeners.forEach((l) => l());
    })
    .catch(() => {});
}

/** Applies a notice to the messages before it in `buf` (by time). */
function applyNotice(buf: ChatMessage[], n: ChatMessage) {
  if (!n.notice) return;
  for (const m of buf) {
    if (m.notice || m.ts > n.ts) continue;
    const hit =
      n.notice.kind === "clear" ||
      (n.notice.kind === "delete" ? m.id === n.notice.target : m.user === n.user);
    if (!hit) continue;
    m.deleted = true;
    // The notice only knows the login; their messages carry how the name is written.
    if (n.notice.kind !== "clear" && m.user === n.user) n.display_name = m.display_name;
  }
}

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
    else if (data.type === "chat_history") receiveHistory(data.channel, data.messages);
    else eventListeners.forEach((l) => l(data as ServerEvent));
  };
  socket.onclose = () => {
    socket = null;
    if (wanted.length > 0 || eventListeners.size > 0) setTimeout(connect, (retry = Math.min(retry * 2, 30_000)));
  };
}

/** Order of display: Twitch's send time, then arrival. */
const byTime = (a: ChatMessage, b: ChatMessage) => a.ts - b.ts || a.seq - b.seq;
const sameMessage = (a: ChatMessage, b: ChatMessage) => a.ts === b.ts && a.user === b.user && a.text === b.text;

function receive(m: ChatMessage) {
  const buf = buffers.get(m.channel);
  if (!buf) return;
  const msg = { ...m, ts: m.ts || Date.now(), seq: seq++ };
  applyNotice(buf, msg);
  // A deletion only hides its message; it needs no line of its own.
  if (msg.notice?.kind !== "delete") buf.push(msg);
  if (buf.length > KEEP) buf.splice(0, buf.length - KEEP);
  changed();
}

/** Messages from before the chat was opened; merged in without duplicates. */
function receiveHistory(channel: string, messages: ChatMessage[]) {
  const buf = buffers.get(channel);
  if (!buf || messages.length === 0) return;
  for (const m of messages) if (!buf.some((b) => sameMessage(b, m))) buf.push({ ...m, seq: seq++ });
  buf.sort(byTime);
  for (const n of buf.filter((m) => m.notice)) applyNotice(buf, n);
  for (let i = buf.length - 1; i >= 0; i--) if (buf[i].notice?.kind === "delete") buf.splice(i, 1);
  if (buf.length > KEEP) buf.splice(0, buf.length - KEEP);
  changed();
}

function changed() {
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
  for (const c of buffers.keys())
    if (!next.includes(c)) {
      buffers.delete(c);
      emoteSets.delete(c);
    }
  for (const c of next)
    if (!buffers.has(c)) {
      buffers.set(c, []);
      loadEmotes(c);
    }
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
        .sort(byTime)
        .slice(-KEEP);
    }
    return cache;
  });
}

export type Part = { text: string } | { src: string; srcset?: string; name: string };

const twitchEmote = (id: string, name: string): Part => ({
  src: `https://static-cdn.jtvnw.net/emoticons/v2/${id}/default/dark/1.0`,
  srcset: `https://static-cdn.jtvnw.net/emoticons/v2/${id}/default/dark/2.0 2x`,
  name,
});

/** Words that are 7TV emotes of the message's channel become images. */
function withThirdParty(text: string, set: Record<string, string> | undefined): Part[] {
  if (!set) return [{ text }];
  const out: Part[] = [];
  let pending = "";
  for (const piece of text.split(/(\s+)/)) {
    const id = piece && set[piece];
    if (id) {
      if (pending) out.push({ text: pending });
      pending = "";
      out.push({ src: `https://cdn.7tv.app/emote/${id}/1x.webp`, srcset: `https://cdn.7tv.app/emote/${id}/2x.webp 2x`, name: piece });
    } else pending += piece;
  }
  if (pending) out.push({ text: pending });
  return out;
}

/**
 * Splits a message into text and emote parts: Twitch's emotes by position (code points, not
 * UTF-16), then 7TV's by word (unless `thirdParty` is off).
 */
export function parts(m: ChatMessage, thirdParty = true): Part[] {
  const set = thirdParty ? emoteSets.get(m.channel) : undefined;
  if (m.emotes.length === 0) return withThirdParty(m.text, set);
  const chars = Array.from(m.text);
  const sorted = [...m.emotes].sort((a, b) => a[1] - b[1]);
  const out: Part[] = [];
  let pos = 0;
  for (const [id, start, end] of sorted) {
    if (start < pos || end >= chars.length) continue;
    if (start > pos) out.push(...withThirdParty(chars.slice(pos, start).join(""), set));
    out.push(twitchEmote(id, chars.slice(start, end + 1).join("")));
    pos = end + 1;
  }
  if (pos < chars.length) out.push(...withThirdParty(chars.slice(pos).join(""), set));
  return out;
}

/** A notice as text: "spammer was timed out (10 min)". */
export function noticeText(m: ChatMessage): string {
  const n = m.notice;
  if (!n) return "";
  if (n.kind === "clear") return "The chat was cleared by a moderator";
  if (n.kind === "ban") return `${m.display_name} was banned`;
  if (n.kind === "timeout") {
    const s = n.seconds;
    const span = s < 60 ? `${s} s` : s < 3600 ? `${Math.round(s / 60)} min` : s < 86400 ? `${Math.round(s / 3600)} h` : `${Math.round(s / 86400)} d`;
    return `${m.display_name} was timed out (${span})`;
  }
  return "";
}

/** Listens for server events (the socket stays open while anyone listens). */
export function watchEvents(onEvent: (e: ServerEvent) => void): () => void {
  eventListeners.add(onEvent);
  if (!socket) connect();
  return () => {
    eventListeners.delete(onEvent);
  };
}
