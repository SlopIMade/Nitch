import { useEffect, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { api } from "../api";
import { usePlayer } from "../player/PlayerContext";

function useDebounced(value: string, ms: number): string {
  const [v, setV] = useState(value);
  useEffect(() => {
    const t = setTimeout(() => setV(value), ms);
    return () => clearTimeout(t);
  }, [value, ms]);
  return v;
}

export function SearchPage() {
  const { play } = usePlayer();
  const [text, setText] = useState("");
  const q = useDebounced(text.trim(), 300);
  const results = useQuery({ queryKey: ["search", q], queryFn: () => api.search(q), enabled: q.length > 0 });

  return (
    <div className="page">
      <input
        className="input input--search"
        placeholder="Search channels"
        value={text}
        onChange={(e) => setText(e.target.value)}
        autoFocus
      />
      {results.isError && <p className="error">Search failed: {results.error.message}</p>}
      {results.data?.length === 0 && <p className="muted">No channels found.</p>}
      <ul className="results">
        {results.data?.map((c) => (
          <li key={c.login}>
            <button className="result" disabled={!c.is_live} onClick={() => play(c.login)}>
              <img className="avatar" src={c.avatar_url} alt="" loading="lazy" />
              <div className="result__text">
                <div>
                  <strong>{c.display_name}</strong>{" "}
                  {c.is_live ? <span className="badge badge--live">LIVE</span> : <span className="muted">offline</span>}
                </div>
                {c.is_live && (
                  <div className="muted result__title">
                    {c.game} · {c.title}
                  </div>
                )}
              </div>
            </button>
          </li>
        ))}
      </ul>
    </div>
  );
}
