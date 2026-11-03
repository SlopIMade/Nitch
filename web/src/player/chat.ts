// Chat data: one WebSocket to the server for the whole app (D6, M6). Components say which
// channels they show; the connection subscribes to exactly those and reconnects on its own.

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
};

type Listener = (m: ChatMessage) => void;

let socket: WebSocket | null = null;
let wanted: string[] = [];
const listeners = new Set<Listener>();
let retry = 1000;

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
    if (data.type === "chat") listeners.forEach((l) => l(data as ChatMessage));
  };
  socket.onclose = () => {
    socket = null;
    if (listeners.size > 0) setTimeout(connect, (retry = Math.min(retry * 2, 30_000)));
  };
}

/** Shows `channels`' chat; returns the function that stops listening. */
export function watchChat(channels: string[], onMessage: Listener): () => void {
  listeners.add(onMessage);
  wanted = channels;
  if (!socket) connect();
  else send();
  return () => {
    listeners.delete(onMessage);
    if (listeners.size === 0) {
      wanted = [];
      send();
    }
  };
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
