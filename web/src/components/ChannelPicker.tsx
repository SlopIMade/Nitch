// Picks a channel for a multiview tile: followed channels that are live, plus search.

import { useEffect, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { api } from "../api";

export function ChannelPicker({ onPick, onClose }: { onPick: (login: string) => void; onClose: () => void }) {
  const [text, setText] = useState("");
  const [q, setQ] = useState("");
  useEffect(() => {
    const t = setTimeout(() => setQ(text.trim()), 300);
    return () => clearTimeout(t);
  }, [text]);
  const auth = useQuery({ queryKey: ["auth"], queryFn: api.auth });
  const follows = useQuery({ queryKey: ["follows"], queryFn: api.followsLive, enabled: auth.data?.state === "logged_in" });
  const results = useQuery({ queryKey: ["search", q], queryFn: () => api.search(q), enabled: q.length > 0 });

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const list = q
    ? (results.data ?? []).map((c) => ({ login: c.login, name: c.display_name, sub: c.is_live ? c.game : "offline", live: c.is_live }))
    : (follows.data ?? []).map((s) => ({ login: s.login, name: s.display_name, sub: s.game, live: true }));

  return (
    <div className="modal" onClick={onClose}>
      <div className="modal__box" onClick={(e) => e.stopPropagation()}>
        <div className="row row--spread">
          <h2>Add a stream</h2>
          <button className="btn btn--ghost" onClick={onClose}>
            ✕
          </button>
        </div>
        <input className="input input--search" placeholder="Search channels" value={text} onChange={(e) => setText(e.target.value)} autoFocus />
        {!q && <p className="muted">{follows.data ? "Live from the channels you follow:" : "Search for a channel."}</p>}
        <ul className="results">
          {list.map((c) => (
            <li key={c.login}>
              <button className="result" onClick={() => onPick(c.login)}>
                <div className="result__text">
                  <strong>{c.name}</strong> {c.live && <span className="badge badge--live">LIVE</span>}
                  <div className="muted result__title">{c.sub}</div>
                </div>
              </button>
            </li>
          ))}
        </ul>
      </div>
    </div>
  );
}
