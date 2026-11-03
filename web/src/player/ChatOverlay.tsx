// Chat overlay (M6, D12): drawn by the client over the video, so it stays sharp and changes
// instantly. Modes: off, the focused stream, or one combined window of several streams.
// Position, size (percent of the 16:9 stage), font size and opacity are saved per device and
// change live. Mouse: drag the header, drag the corner. Remote/keyboard: "Move" or "Size",
// then the arrows; OK or Back to finish.

import { useEffect, useRef, useState, type PointerEvent as ReactPointerEvent } from "react";
import { claimArrows } from "../tv";
import { parts, watchChat, type ChatMessage } from "./chat";

type Mode = "off" | "focused" | "combined";
type Settings = { mode: Mode; x: number; y: number; w: number; h: number; font: number; opacity: number; exclude: string[] };

const KEY = "nitch.chat";
const DEFAULTS: Settings = { mode: "focused", x: 73, y: 4, w: 25, h: 72, font: 15, opacity: 0.55, exclude: [] };
const KEEP = 150;
const clamp = (v: number, lo: number, hi: number) => Math.max(lo, Math.min(hi, v));

function load(): Settings {
  try {
    return { ...DEFAULTS, ...JSON.parse(localStorage.getItem(KEY) ?? "{}") };
  } catch {
    return DEFAULTS;
  }
}

/** A stable color per channel for the stripe in the combined chat. */
function channelColor(channel: string): string {
  let h = 0;
  for (const c of channel) h = (h * 31 + c.charCodeAt(0)) % 360;
  return `hsl(${h} 70% 60%)`;
}

export function ChatOverlay({ focused, all }: { focused: string | null; all: string[] }) {
  const [s, setS] = useState<Settings>(load);
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [adjust, setAdjust] = useState<"move" | "size" | null>(null);
  const box = useRef<HTMLDivElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const update = (patch: Partial<Settings>) =>
    setS((old) => {
      const next = { ...old, ...patch };
      localStorage.setItem(KEY, JSON.stringify(next));
      return next;
    });

  const channels =
    s.mode === "off" ? [] : s.mode === "focused" ? (focused ? [focused] : []) : all.filter((c) => !s.exclude.includes(c));
  const key = channels.join(",");

  // Subscribe to the shown channels; batch incoming messages into one update per frame.
  useEffect(() => {
    if (channels.length === 0) return;
    setMessages([]);
    let pending: ChatMessage[] = [];
    let frame = 0;
    const stop = watchChat(channels, (m) => {
      pending.push(m);
      if (!frame)
        frame = requestAnimationFrame(() => {
          const batch = pending;
          pending = [];
          frame = 0;
          setMessages((old) => [...old, ...batch].slice(-KEEP));
        });
    });
    return () => {
      stop();
      cancelAnimationFrame(frame);
    };
    // `key` stands for `channels`.
  }, [key]);

  // Stick to the newest message.
  useEffect(() => {
    const el = list.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [messages]);

  // Adjusting with arrows (remote): the chat claims the arrows until OK, Back or Escape.
  useEffect(() => {
    if (!adjust) return;
    const release = claimArrows();
    const onKey = (e: KeyboardEvent) => {
      const d = { ArrowLeft: [-1, 0], ArrowRight: [1, 0], ArrowUp: [0, -1], ArrowDown: [0, 1] }[e.key];
      if (d) {
        e.preventDefault();
        setS((old) => {
          const next =
            adjust === "move"
              ? { ...old, x: clamp(old.x + d[0], 0, 100 - old.w), y: clamp(old.y + d[1], 0, 100 - old.h) }
              : { ...old, w: clamp(old.w + d[0], 12, 100 - old.x), h: clamp(old.h + d[1], 15, 100 - old.y) };
          localStorage.setItem(KEY, JSON.stringify(next));
          return next;
        });
      } else if (e.key === "Enter" || e.key === "Escape") {
        e.preventDefault();
        setAdjust(null);
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => {
      release();
      window.removeEventListener("keydown", onKey, true);
    };
  }, [adjust]);

  // Mouse: drag the header to move, the corner to resize (in percent of the stage).
  const startDrag = (e: ReactPointerEvent, what: "move" | "size") => {
    const stage = box.current?.parentElement?.getBoundingClientRect();
    if (!stage) return;
    e.preventDefault();
    const start = { px: e.clientX, py: e.clientY, s };
    const onMove = (ev: PointerEvent) => {
      const dx = ((ev.clientX - start.px) / stage.width) * 100;
      const dy = ((ev.clientY - start.py) / stage.height) * 100;
      const o = start.s;
      update(
        what === "move"
          ? { x: clamp(o.x + dx, 0, 100 - o.w), y: clamp(o.y + dy, 0, 100 - o.h) }
          : { w: clamp(o.w + dx, 12, 100 - o.x), h: clamp(o.h + dy, 15, 100 - o.y) },
      );
    };
    const onUp = () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
  };

  const modeButton = (
    <select className="select chat__mode" value={s.mode} onChange={(e) => update({ mode: e.target.value as Mode })} aria-label="Chat">
      <option value="off">Chat off</option>
      <option value="focused">Chat: focused stream</option>
      {all.length > 1 && <option value="combined">Chat: combined</option>}
    </select>
  );

  if (s.mode === "off") return <div className="chat chat--off">{modeButton}</div>;

  return (
    <div
      ref={box}
      className={`chat${adjust ? " chat--adjusting" : ""}`}
      style={{ left: `${s.x}%`, top: `${s.y}%`, width: `${s.w}%`, height: `${s.h}%`, fontSize: s.font }}
      onWheel={(e) => {
        // Ctrl + wheel: font size.
        if (e.ctrlKey) {
          e.preventDefault();
          update({ font: clamp(s.font + (e.deltaY < 0 ? 1 : -1), 10, 40) });
        }
      }}
    >
      <div className="chat__bg" style={{ opacity: s.opacity }} />
      <div className="chat__header" onPointerDown={(e) => e.target === e.currentTarget && startDrag(e, "move")}>
        {modeButton}
        <button className="chat__btn" onClick={() => update({ font: clamp(s.font - 1, 10, 40) })} title="Smaller text">
          A−
        </button>
        <button className="chat__btn" onClick={() => update({ font: clamp(s.font + 1, 10, 40) })} title="Larger text">
          A+
        </button>
        <button
          className="chat__btn"
          onClick={() => update({ opacity: Math.round(clamp(s.opacity + (s.opacity >= 0.9 ? -0.8 : 0.15), 0.1, 0.95) * 100) / 100 })}
          title="Background opacity"
        >
          ◐
        </button>
        <button className={`chat__btn${adjust === "move" ? " chat__btn--on" : ""}`} onClick={() => setAdjust(adjust === "move" ? null : "move")}>
          Move
        </button>
        <button className={`chat__btn${adjust === "size" ? " chat__btn--on" : ""}`} onClick={() => setAdjust(adjust === "size" ? null : "size")}>
          Size
        </button>
      </div>
      {s.mode === "combined" && all.length > 1 && (
        <div className="chat__channels">
          {all.map((c) => (
            <button
              key={c}
              className={`chat__chip${s.exclude.includes(c) ? " chat__chip--off" : ""}`}
              style={{ borderColor: channelColor(c) }}
              onClick={() => update({ exclude: s.exclude.includes(c) ? s.exclude.filter((x) => x !== c) : [...s.exclude, c] })}
            >
              {c}
            </button>
          ))}
        </div>
      )}
      <div className="chat__list" ref={list}>
        {channels.length === 0 && <div className="chat__empty">No stream selected.</div>}
        {messages.map((m, i) => (
          <div key={i} className="chat__line" style={s.mode === "combined" ? { borderLeftColor: channelColor(m.channel) } : undefined}>
            <span className="chat__name" style={{ color: m.color || "#a970ff" }}>
              {m.display_name}
            </span>
            <span className="chat__sep">: </span>
            {parts(m).map((p, j) =>
              "emote" in p ? (
                <img key={j} className="chat__emote" alt={p.name} title={p.name} src={`https://static-cdn.jtvnw.net/emoticons/v2/${p.emote}/default/dark/1.0`} />
              ) : (
                <span key={j}>{p.text}</span>
              ),
            )}
          </div>
        ))}
      </div>
      <div className="chat__resize" onPointerDown={(e) => startDrag(e, "size")} title="Resize" />
      {adjust && <div className="chat__hint">{adjust === "move" ? "Arrows move" : "Arrows resize"} · OK to finish</div>}
    </div>
  );
}
