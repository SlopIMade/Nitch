// Saved multiview sessions (D17): the same list on every device.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate } from "react-router";
import { api, GRID, type Rect } from "../api";
import { usePlayer } from "../player/PlayerContext";

/** Tiny drawing of a layout on the 16:9 canvas. */
export function LayoutPreview({ rects, labels }: { rects: Rect[]; labels?: string[] }) {
  const pct = (n: number) => `${(n / GRID) * 100}%`;
  return (
    <div className="layout-preview">
      {rects.map((r, i) => (
        <div key={i} className="layout-preview__tile" style={{ left: pct(r.x), top: pct(r.y), width: pct(r.w), height: pct(r.h) }}>
          {labels?.[i]}
        </div>
      ))}
    </div>
  );
}

export function SessionsPage() {
  const qc = useQueryClient();
  const navigate = useNavigate();
  const { playSession } = usePlayer();
  const sessions = useQuery({ queryKey: ["sessions"], queryFn: api.sessions });
  const templates = useQuery({ queryKey: ["templates"], queryFn: api.templates, staleTime: Infinity });
  const remove = useMutation({
    mutationFn: api.deleteSession,
    onSuccess: () => qc.invalidateQueries({ queryKey: ["sessions"] }),
  });

  return (
    <div className="page">
      <h2>Multiview</h2>
      {sessions.isError && <p className="error">Could not load sessions: {sessions.error.message}</p>}
      {sessions.data?.length === 0 && <p className="muted">No sessions yet. Start one from a layout below.</p>}
      <div className="grid">
        {sessions.data?.map((s) => (
          <div key={s.id} className="card card--static">
            <button className="card__thumb card__thumb--layout" onClick={() => playSession(s.id, s.name)} title="Play">
              <LayoutPreview rects={s.tiles} labels={s.tiles.map((t) => t.channel)} />
            </button>
            <div className="card__body">
              <div className="card__title">{s.name}</div>
              <div className="row row--left">
                <button className="btn btn--small" onClick={() => playSession(s.id, s.name)}>
                  ▶ Play
                </button>
                <Link className="btn btn--ghost btn--small" to={`/multiview/${s.id}`}>
                  Edit
                </Link>
                <button
                  className="btn btn--ghost btn--small"
                  onClick={() => confirm(`Delete "${s.name}"?`) && remove.mutate(s.id)}
                >
                  Delete
                </button>
              </div>
            </div>
          </div>
        ))}
      </div>

      <h2 className="section">New session</h2>
      <div className="grid grid--small">
        {templates.data?.map((t) => (
          <button key={t.key} className="card" onClick={() => navigate(`/multiview/new?template=${t.key}`)}>
            <div className="card__thumb card__thumb--layout">
              <LayoutPreview rects={t.rects} />
            </div>
            <div className="card__body">
              <div className="card__title">{t.name}</div>
            </div>
          </button>
        ))}
      </div>
    </div>
  );
}
