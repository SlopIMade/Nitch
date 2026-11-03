// The player's side menu: add streams to what's playing, and the chat settings. Opened with a
// long press of Left on the remote or the menu button (mouse). For the remote, OK on a stream
// watches it alone, a long press of OK (or the + button) adds it to the view.

import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { useQueries, useQuery } from "@tanstack/react-query";
import { api, formatViewers } from "../api";
import { Icon } from "../components/Icon";
import { installBackHandler, pressKind } from "../tv";
import { CHAT_PLACES, channelColor, resetChat, updateChat, useChatSettings, type ChatMode } from "./chatSettings";
import type { ChatAdjust } from "./ChatOverlay";

export type MenuTab = "add" | "chat";

/** `@name` mentions in stream titles: the people a streamer plays with. */
export function mentions(titles: string[]): string[] {
  const found = titles.flatMap((t) => [...t.matchAll(/@([A-Za-z0-9_]{3,25})/g)].map((m) => m[1].toLowerCase()));
  return [...new Set(found)];
}

/** A button that tells OK from a long press of OK (remote) and has an optional side action (mouse). */
function Entry({
  onShort,
  onLong,
  longLabel,
  children,
  disabled,
}: {
  onShort: () => void;
  onLong?: () => void;
  longLabel?: string;
  children: ReactNode;
  disabled?: boolean;
}) {
  return (
    <div className="menu-entry">
      <button
        className="menu-entry__main"
        disabled={disabled}
        onClick={(e) => e.detail > 0 && onShort()}
        onKeyDown={(e) => pressKind(e, "Enter", onLong ?? onShort)}
        onKeyUp={(e) => pressKind(e, "Enter") === "short" && onShort()}
      >
        {children}
      </button>
      {onLong && (
        <button className="menu-entry__side" onClick={onLong} disabled={disabled} title={longLabel} aria-label={longLabel}>
          <Icon name={longLabel === "Remove" ? "trash" : "plus"} />
        </button>
      )}
    </div>
  );
}

type Stream = { login: string; name: string; sub: string; live: boolean; viewers?: number };

function StreamList({
  title,
  streams,
  onWatch,
  onAdd,
  canAdd,
}: {
  title: string;
  streams: Stream[];
  onWatch: (login: string) => void;
  onAdd: (login: string) => void;
  canAdd: boolean;
}) {
  if (streams.length === 0) return null;
  return (
    <section className="sidemenu__section">
      <h3>{title}</h3>
      {streams.map((s) => (
        <Entry
          key={s.login}
          disabled={!s.live}
          onShort={() => onWatch(s.login)}
          onLong={canAdd ? () => onAdd(s.login) : undefined}
          longLabel="Add to this view"
        >
          <strong>{s.name}</strong>
          <span className="menu-entry__sub">
            {s.live ? s.sub : "offline"}
            {s.viewers !== undefined && (
              <span className="menu-entry__viewers">
                <Icon name="viewers" /> {formatViewers(s.viewers)}
              </span>
            )}
          </span>
        </Entry>
      ))}
    </section>
  );
}

function AddTab({
  inView,
  titles,
  onWatch,
  onAdd,
  onRemove,
  onFocusTile,
  onEditLayout,
  full,
}: {
  inView: string[];
  titles: string[];
  onWatch: (login: string) => void;
  onAdd: (login: string) => void;
  onRemove: ((channel: string) => void) | null;
  onFocusTile: (channel: string) => void;
  onEditLayout: (() => void) | null;
  full: boolean;
}) {
  const [text, setText] = useState("");
  const [q, setQ] = useState("");
  useEffect(() => {
    const t = setTimeout(() => setQ(text.trim()), 300);
    return () => clearTimeout(t);
  }, [text]);
  const auth = useQuery({ queryKey: ["auth"], queryFn: api.auth });
  const follows = useQuery({ queryKey: ["follows"], queryFn: api.followsLive, enabled: auth.data?.state === "logged_in" });
  const results = useQuery({ queryKey: ["search", q], queryFn: () => api.search(q), enabled: q.length > 0 });
  const tagged = mentions(titles).filter((c) => !inView.includes(c));
  const tagInfo = useQueries({
    queries: tagged.map((login) => ({ queryKey: ["channel", login], queryFn: () => api.channel(login), staleTime: 60_000 })),
  });

  const notInView = (s: Stream) => !inView.includes(s.login);
  const found: Stream[] = (results.data ?? [])
    .map((c) => ({ login: c.login, name: c.display_name, sub: c.game, live: c.is_live }))
    .filter(notInView);
  const recommended: Stream[] = tagInfo.flatMap((t) =>
    t.data?.live ? [{ login: t.data.login, name: t.data.display_name, sub: t.data.live.game, live: true, viewers: t.data.live.viewers }] : [],
  );
  const followed: Stream[] = (follows.data ?? [])
    .map((s) => ({ login: s.login, name: s.display_name, sub: s.game, live: true, viewers: s.viewers }))
    .filter(notInView)
    .filter((s) => !recommended.some((r) => r.login === s.login));

  return (
    <>
      <input className="input sidemenu__search" placeholder="Search channels" value={text} onChange={(e) => setText(e.target.value)} />
      <p className="sidemenu__hint">
        OK: watch alone · hold OK or <Icon name="plus" />: add to this view{full ? " (full)" : ""}
      </p>
      {q && <StreamList title="Search results" streams={found} onWatch={onWatch} onAdd={onAdd} canAdd={!full} />}
      {q && results.data?.length === 0 && <p className="muted">No channels found.</p>}
      <StreamList title="Recommended" streams={recommended} onWatch={onWatch} onAdd={onAdd} canAdd={!full} />
      <StreamList title="Followed, live" streams={followed} onWatch={onWatch} onAdd={onAdd} canAdd={!full} />
      {inView.length > 0 && (
        <section className="sidemenu__section">
          <h3>In this view</h3>
          {inView.map((c) => (
            <Entry key={c} onShort={() => onFocusTile(c)} onLong={onRemove ? () => onRemove(c) : undefined} longLabel="Remove">
              <strong>{c}</strong>
              <span className="menu-entry__sub">{onRemove ? "OK: listen · hold OK or ✕: remove" : "playing"}</span>
            </Entry>
          ))}
          {onEditLayout && (
            <button className="btn btn--ghost btn--small" onClick={onEditLayout}>
              Edit layout…
            </button>
          )}
        </section>
      )}
    </>
  );
}

function ChatTab({ all, onAdjust }: { all: string[]; onAdjust: (a: ChatAdjust) => void }) {
  const s = useChatSettings();
  const modes: [ChatMode, string][] = [
    ["off", "Off"],
    ["focused", all.length > 1 ? "Focused stream" : "On"],
    ...(all.length > 1 ? ([["combined", "All streams"]] as [ChatMode, string][]) : []),
  ];
  return (
    <>
      <section className="sidemenu__section">
        <h3>Chat</h3>
        <div className="seg">
          {modes.map(([m, label]) => (
            <button key={m} className={`seg__btn${s.mode === m ? " seg__btn--on" : ""}`} onClick={() => updateChat({ mode: m })}>
              {label}
            </button>
          ))}
        </div>
      </section>
      {s.mode === "combined" && all.length > 1 && (
        <section className="sidemenu__section">
          <h3>Channels</h3>
          <div className="chips">
            {all.map((c) => (
              <button
                key={c}
                className={`chip${s.exclude.includes(c) ? " chip--off" : ""}`}
                style={{ borderColor: channelColor(c) }}
                onClick={() => updateChat({ exclude: s.exclude.includes(c) ? s.exclude.filter((x) => x !== c) : [...s.exclude, c] })}
              >
                {c}
              </button>
            ))}
          </div>
        </section>
      )}
      <section className="sidemenu__section">
        <h3>Look</h3>
        <div className="stepper">
          <span>Text</span>
          <button className="btn btn--ghost btn--small" onClick={() => updateChat((o) => ({ font: o.font - 0.05 }))}>
            A−
          </button>
          <span className="stepper__value">{Math.round(s.font * 100)} %</span>
          <button className="btn btn--ghost btn--small" onClick={() => updateChat((o) => ({ font: o.font + 0.05 }))}>
            A+
          </button>
        </div>
        <div className="stepper">
          <span>Text opacity</span>
          <button className="btn btn--ghost btn--small" onClick={() => updateChat((o) => ({ text: o.text - 0.1 }))}>
            −
          </button>
          <span className="stepper__value">{Math.round(s.text * 100)} %</span>
          <button className="btn btn--ghost btn--small" onClick={() => updateChat((o) => ({ text: o.text + 0.1 }))}>
            +
          </button>
        </div>
        <div className="stepper">
          <span>Background</span>
          <button className="btn btn--ghost btn--small" onClick={() => updateChat((o) => ({ opacity: o.opacity - 0.1 }))}>
            −
          </button>
          <span className="stepper__value">{Math.round(s.opacity * 100)} %</span>
          <button className="btn btn--ghost btn--small" onClick={() => updateChat((o) => ({ opacity: o.opacity + 0.1 }))}>
            +
          </button>
        </div>
      </section>
      <section className="sidemenu__section">
        <h3>Place</h3>
        <div className="chips">
          {CHAT_PLACES.map((p) => (
            <button key={p.label} className="chip" onClick={() => updateChat(p.rect)}>
              {p.label}
            </button>
          ))}
        </div>
        <div className="chips">
          <button className="chip" onClick={() => onAdjust("move")}>
            Move with arrows
          </button>
          <button className="chip" onClick={() => onAdjust("size")}>
            Resize with arrows
          </button>
          <button className="chip" onClick={resetChat}>
            Reset
          </button>
        </div>
        <p className="sidemenu__hint">With a mouse: drag the chat's top edge or its corner; Ctrl + wheel for text size.</p>
      </section>
    </>
  );
}

export function SideMenu(props: {
  tab: MenuTab;
  onTab: (t: MenuTab) => void;
  onClose: () => void;
  inView: string[];
  titles: string[];
  onWatch: (login: string) => void;
  onAdd: (login: string) => void;
  onRemove: ((channel: string) => void) | null;
  onFocusTile: (channel: string) => void;
  onEditLayout: (() => void) | null;
  onAdjustChat: (a: ChatAdjust) => void;
  full: boolean;
}) {
  const { tab, onTab, onClose } = props;
  const box = useRef<HTMLDivElement>(null);

  // Back or Escape closes the menu (before anything else handles them).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        onClose();
      }
    };
    const removeBack = installBackHandler(() => {
      onClose();
      return true;
    });
    window.addEventListener("keydown", onKey, true);
    return () => {
      removeBack();
      window.removeEventListener("keydown", onKey, true);
    };
  }, [onClose]);

  // The focus belongs in the menu: on opening, on switching tabs, and whenever it got lost (a
  // list that loaded late, an entry that went away), it goes to the first entry.
  const focusFirst = () => {
    const first = box.current?.querySelector<HTMLElement>(".sidemenu__body button:not(:disabled)");
    (first ?? box.current?.querySelector<HTMLElement>(".sidemenu__tab--on"))?.focus();
  };
  // Until the user moves, the focus follows the first entry (lists load in after opening).
  const moved = useRef(false);
  useLayoutEffect(() => {
    moved.current = false;
    focusFirst();
  }, [tab]);
  useEffect(() => {
    if (!moved.current || !box.current?.contains(document.activeElement)) focusFirst();
  });
  // An arrow while the focus is somewhere else (it can't be in the menu then): select the first
  // entry instead of going nowhere.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!e.key.startsWith("Arrow") || box.current?.contains(document.activeElement)) return;
      e.preventDefault();
      e.stopImmediatePropagation();
      focusFirst();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, []);

  return (
    <div
      className="sidemenu"
      ref={box}
      onClick={(e) => e.stopPropagation()}
      onKeyDown={(e) => {
        if (e.key.startsWith("Arrow")) moved.current = true;
      }}
      onPointerDown={() => (moved.current = true)}
    >
      <div className="sidemenu__tabs">
        <button className={`sidemenu__tab${tab === "add" ? " sidemenu__tab--on" : ""}`} onClick={() => onTab("add")}>
          <Icon name="plus" /> Streams
        </button>
        <button className={`sidemenu__tab${tab === "chat" ? " sidemenu__tab--on" : ""}`} onClick={() => onTab("chat")}>
          <Icon name="chat" /> Chat
        </button>
        <button className="btn btn--ghost btn--icon sidemenu__close" onClick={onClose} title="Close (Back)">
          <Icon name="close" />
        </button>
      </div>
      <div className="sidemenu__body">
        {tab === "add" ? <AddTab {...props} /> : <ChatTab all={props.inView} onAdjust={props.onAdjustChat} />}
      </div>
    </div>
  );
}
