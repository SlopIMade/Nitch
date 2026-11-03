// Chat overlay (M6, D12): drawn by the client over the video, so it stays sharp and changes
// instantly. Modes: off, the focused stream, or one combined window of several streams.
// It can sit anywhere on the screen, black bars included. Settings live in chatSettings.ts and
// change from the side menu (remote) or here with the mouse: drag the top edge to move, the
// corner to resize, Ctrl + wheel for the text size. For the remote, `adjust` turns the arrows
// into move/resize until OK or Back.

import { useEffect, useRef, type PointerEvent as ReactPointerEvent } from "react";
import { claimArrows } from "../tv";
import { parts, useChatMessages } from "./chat";
import { channelColor, updateChat, useChatSettings, type ChatSettings } from "./chatSettings";

export type ChatAdjust = "move" | "size" | null;

/** The channels the chat shows, given the settings and what's on screen. */
export function shownChannels(s: ChatSettings, focused: string | null, all: string[]): string[] {
  if (s.mode === "off") return [];
  if (s.mode === "focused" || all.length < 2) return focused ? [focused] : [];
  return all.filter((c) => !s.exclude.includes(c));
}

export function ChatOverlay({
  focused,
  all,
  adjust,
  onAdjustDone,
  onSettings,
}: {
  focused: string | null;
  all: string[];
  adjust: ChatAdjust;
  onAdjustDone: () => void;
  onSettings: () => void;
}) {
  const s = useChatSettings();
  const channels = shownChannels(s, focused, all);
  const messages = useChatMessages(channels);
  const list = useRef<HTMLDivElement>(null);
  const box = useRef<HTMLDivElement>(null);
  const mixed = channels.length > 1;

  // Stick to the newest message.
  useEffect(() => {
    const el = list.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [messages]);

  // Remote: the arrows move or resize the chat until OK, Back or Escape.
  useEffect(() => {
    if (!adjust) return;
    const release = claimArrows();
    const onKey = (e: KeyboardEvent) => {
      const d = { ArrowLeft: [-1, 0], ArrowRight: [1, 0], ArrowUp: [0, -1], ArrowDown: [0, 1] }[e.key];
      if (d) {
        e.preventDefault();
        e.stopPropagation();
        const step = e.repeat ? 2 : 1;
        updateChat((o) =>
          adjust === "move" ? { x: o.x + d[0] * step, y: o.y + d[1] * step } : { w: o.w + d[0] * step, h: o.h + d[1] * step },
        );
      } else if (e.key === "Enter" || e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        onAdjustDone();
      }
    };
    const onBack = (e: Event) => {
      e.stopImmediatePropagation();
      onAdjustDone();
    };
    window.addEventListener("keydown", onKey, true);
    window.addEventListener("nitch:back", onBack, true);
    return () => {
      release();
      window.removeEventListener("keydown", onKey, true);
      window.removeEventListener("nitch:back", onBack, true);
    };
  }, [adjust, onAdjustDone]);

  // Mouse: drag the top edge to move, the corner to resize (in percent of the screen).
  const startDrag = (e: ReactPointerEvent, what: "move" | "size") => {
    const screen = box.current?.parentElement?.getBoundingClientRect();
    if (!screen) return;
    e.preventDefault();
    const start = { px: e.clientX, py: e.clientY, s };
    const onMove = (ev: PointerEvent) => {
      const dx = ((ev.clientX - start.px) / screen.width) * 100;
      const dy = ((ev.clientY - start.py) / screen.height) * 100;
      const o = start.s;
      updateChat(what === "move" ? { x: o.x + dx, y: o.y + dy } : { w: o.w + dx, h: o.h + dy });
    };
    const onUp = () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
  };

  if (s.mode === "off" && !adjust) return null;

  return (
    <div
      ref={box}
      className={`chat${adjust ? " chat--adjusting" : ""}`}
      style={{ left: `${s.x}%`, top: `${s.y}%`, width: `${s.w}%`, height: `${s.h}%`, fontSize: `${s.font}rem` }}
      onWheel={(e) => {
        if (e.ctrlKey) {
          e.preventDefault();
          updateChat((o) => ({ font: o.font + (e.deltaY < 0 ? 0.05 : -0.05) }));
        }
      }}
    >
      <div className="chat__bg" style={{ opacity: s.opacity }} />
      <div className="chat__grip" onPointerDown={(e) => startDrag(e, "move")} title="Drag to move">
        {!mixed && channels[0] && <span className="chat__title">{channels[0]}</span>}
        <button
          className="chat__gear"
          onPointerDown={(e) => e.stopPropagation()}
          onClick={onSettings}
          title="Chat settings"
          tabIndex={-1}
        >
          ⚙
        </button>
      </div>
      <div className="chat__list" ref={list}>
        {channels.length === 0 && <div className="chat__empty">Chat is off.</div>}
        {messages.map((m) => (
          <div key={m.seq} className="chat__line" style={mixed ? { borderLeftColor: channelColor(m.channel) } : undefined}>
            {mixed && (
              <span className="chat__channel" style={{ color: channelColor(m.channel) }}>
                {m.channel}
              </span>
            )}
            <span className="chat__name" style={{ color: m.color || "#a970ff" }}>
              {m.display_name}
            </span>
            <span className="chat__sep">: </span>
            {parts(m).map((p, j) =>
              "emote" in p ? (
                <img
                  key={j}
                  className="chat__emote"
                  alt={p.name}
                  title={p.name}
                  src={`https://static-cdn.jtvnw.net/emoticons/v2/${p.emote}/default/dark/1.0`}
                />
              ) : (
                <span key={j}>{p.text}</span>
              ),
            )}
          </div>
        ))}
      </div>
      <div className="chat__resize" onPointerDown={(e) => startDrag(e, "size")} title="Drag to resize" />
      {adjust && <div className="chat__hint">{adjust === "move" ? "Arrows move the chat" : "Arrows resize the chat"} · OK when done</div>}
    </div>
  );
}
