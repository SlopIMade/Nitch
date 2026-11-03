// Grid editor for a multiview session (D13/D17): tiles snap to the 60×60 grid. Drag to move,
// drag the corner to resize (keeps 16:9, i.e. equal width and height in cells; Shift for free
// size). Keyboard: arrows move the selected tile, Shift+arrows resize it, Delete removes it.

import { useEffect, useMemo, useRef, useState, type PointerEvent as ReactPointerEvent } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useNavigate, useParams, useSearchParams } from "react-router";
import { api, GRID, type Tile } from "../api";
import { ChannelPicker } from "../components/ChannelPicker";
import { usePlayer } from "../player/PlayerContext";

type EditorTile = { channel: string | null; x: number; y: number; w: number; h: number; z: number };
type Drag = { index: number; mode: "move" | "resize"; sx: number; sy: number; start: EditorTile; free: boolean };

const MIN = 6;
const clamp = (v: number, lo: number, hi: number) => Math.max(lo, Math.min(hi, v));

/** Keeps a tile inside the grid with at least MIN cells per side. */
function fit(t: EditorTile): EditorTile {
  const w = clamp(t.w, MIN, GRID);
  const h = clamp(t.h, MIN, GRID);
  return { ...t, w, h, x: clamp(t.x, 0, GRID - w), y: clamp(t.y, 0, GRID - h) };
}

export function SessionEditorPage() {
  const { id } = useParams();
  const [params] = useSearchParams();
  const isNew = !id || id === "new";
  const navigate = useNavigate();
  const qc = useQueryClient();
  const { playSession } = usePlayer();
  const session = useQuery({ queryKey: ["session", id], queryFn: () => api.session(id!), enabled: !isNew });
  const templates = useQuery({ queryKey: ["templates"], queryFn: api.templates, staleTime: Infinity, enabled: isNew });

  const [name, setName] = useState("");
  const [tiles, setTiles] = useState<EditorTile[] | null>(null);
  const [selected, setSelected] = useState<number | null>(null);
  const [picking, setPicking] = useState<number | "new" | null>(null);
  const canvas = useRef<HTMLDivElement>(null);
  const drag = useRef<Drag | null>(null);

  // Initial content: the saved session, or the chosen template with empty tiles.
  useEffect(() => {
    if (tiles) return;
    if (!isNew && session.data) {
      setName(session.data.name);
      setTiles(session.data.tiles.map((t) => ({ ...t })));
    } else if (isNew && templates.data) {
      const t = templates.data.find((t) => t.key === params.get("template")) ?? templates.data[0];
      setName(t ? t.name : "Multiview");
      setTiles((t?.rects ?? []).map((r, i) => ({ ...r, z: i, channel: null })));
    }
  }, [isNew, session.data, templates.data, params, tiles]);

  const update = (i: number, f: (t: EditorTile) => EditorTile) =>
    setTiles((ts) => ts && ts.map((t, k) => (k === i ? fit(f(t)) : t)));
  const remove = (i: number) => {
    setTiles((ts) => ts && ts.filter((_, k) => k !== i));
    setSelected(null);
  };
  const toFront = (i: number) => update(i, (t) => ({ ...t, z: Math.max(0, ...(tiles ?? []).map((o) => o.z)) + 1 }));

  // Keyboard editing (also the way to edit with a TV remote later).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (selected === null || picking !== null || (e.target as HTMLElement).tagName === "INPUT") return;
      const d = { ArrowLeft: [-1, 0], ArrowRight: [1, 0], ArrowUp: [0, -1], ArrowDown: [0, 1] }[e.key];
      if (d) {
        e.preventDefault();
        update(selected, (t) =>
          e.shiftKey ? { ...t, w: t.w + d[0] + d[1], h: t.h + d[0] + d[1] } : { ...t, x: t.x + d[0], y: t.y + d[1] },
        );
      } else if (e.key === "Delete" || e.key === "Backspace") {
        remove(selected);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  const onPointerDown = (e: ReactPointerEvent, index: number, mode: Drag["mode"]) => {
    if (!tiles) return;
    e.stopPropagation();
    (e.target as HTMLElement).setPointerCapture(e.pointerId);
    setSelected(index);
    drag.current = { index, mode, sx: e.clientX, sy: e.clientY, start: tiles[index], free: e.shiftKey };
  };
  const onPointerMove = (e: ReactPointerEvent) => {
    const d = drag.current;
    const box = canvas.current?.getBoundingClientRect();
    if (!d || !box) return;
    const dx = Math.round(((e.clientX - d.sx) / box.width) * GRID);
    const dy = Math.round(((e.clientY - d.sy) / box.height) * GRID);
    update(d.index, () => {
      if (d.mode === "move") return { ...d.start, x: d.start.x + dx, y: d.start.y + dy };
      if (d.free || e.shiftKey) return { ...d.start, w: d.start.w + dx, h: d.start.h + dy };
      const s = Math.max(d.start.w + dx, d.start.h + dy);
      return { ...d.start, w: s, h: s };
    });
  };
  const onPointerUp = () => (drag.current = null);

  const pick = (login: string) => {
    if (picking === "new") {
      const n = tiles?.length ?? 0;
      setTiles((ts) => [...(ts ?? []), fit({ channel: login, x: (n * 5) % 40, y: (n * 5) % 40, w: 20, h: 20, z: n + 1 })]);
      setSelected(n);
    } else if (picking !== null) {
      update(picking, (t) => ({ ...t, channel: login }));
    }
    setPicking(null);
  };

  const complete: Tile[] = useMemo(
    () => (tiles ?? []).filter((t) => t.channel).map((t) => ({ ...t, channel: t.channel!, fit: "contain" as const })),
    [tiles],
  );
  const save = useMutation({
    mutationFn: async (andPlay: boolean) => {
      const s = isNew ? await api.createSession(name, complete) : await api.updateSession(id!, { name, tiles: complete });
      return { s, andPlay };
    },
    onSuccess: ({ s, andPlay }) => {
      qc.invalidateQueries({ queryKey: ["sessions"] });
      qc.setQueryData(["session", s.id], s);
      if (andPlay) playSession(s.id, s.name);
      navigate(`/multiview/${s.id}`, { replace: true });
    },
  });

  if (!tiles) return <div className="page muted">Loading…</div>;
  const pct = (n: number) => `${(n / GRID) * 100}%`;
  const empty = tiles.length - complete.length;

  return (
    <div className="page editor" onPointerMove={onPointerMove} onPointerUp={onPointerUp}>
      <div className="row row--left">
        <input className="input" value={name} onChange={(e) => setName(e.target.value)} maxLength={80} aria-label="Session name" />
        <button className="btn btn--ghost" onClick={() => setPicking("new")} disabled={tiles.length >= 9}>
          + Add stream
        </button>
        {/* Actions for the selected tile live here, so they stay reachable when tiles overlap. */}
        {selected !== null && tiles[selected] && (
          <span className="editor__selection">
            <strong>{tiles[selected].channel ?? "Empty tile"}</strong> {tiles[selected].w}×{tiles[selected].h}
            <button className="btn btn--ghost btn--small" onClick={() => setPicking(selected)}>
              Channel
            </button>
            <button className="btn btn--ghost btn--small" onClick={() => toFront(selected)}>
              To front
            </button>
            <button className="btn btn--ghost btn--small" onClick={() => remove(selected)}>
              Remove
            </button>
          </span>
        )}
        <div className="row-spacer" />
        {empty > 0 && <span className="muted">{empty} tile(s) without a channel won't be saved</span>}
        <button className="btn btn--ghost" disabled={save.isPending || complete.length === 0} onClick={() => save.mutate(false)}>
          Save
        </button>
        <button className="btn" disabled={save.isPending || complete.length === 0} onClick={() => save.mutate(true)}>
          Save & play
        </button>
      </div>
      {save.isError && <p className="error">Saving failed: {save.error.message}</p>}

      <div className="editor__canvas" ref={canvas} onPointerDown={() => setSelected(null)}>
        {tiles.map((t, i) => (
          <div
            key={i}
            className={`editor__tile${i === selected ? " editor__tile--selected" : ""}${t.channel ? "" : " editor__tile--empty"}`}
            style={{ left: pct(t.x), top: pct(t.y), width: pct(t.w), height: pct(t.h), zIndex: t.z + 1 }}
            onPointerDown={(e) => onPointerDown(e, i, "move")}
          >
            <button className="editor__channel" onPointerDown={(e) => e.stopPropagation()} onClick={() => setPicking(i)}>
              {t.channel ?? "+ Choose channel"}
            </button>
            <span className="editor__size">
              {t.w}×{t.h}
            </span>
            <div className="editor__handle" onPointerDown={(e) => onPointerDown(e, i, "resize")} title="Resize (Shift: free)" />
          </div>
        ))}
      </div>
      <p className="muted">
        Drag tiles to move them, drag the corner to resize (keeps the 16:9 shape, Shift for any size). Arrow keys move the selected
        tile, Shift + arrows resize it, Delete removes it. Changes to a running session apply live when you save.
      </p>
      {picking !== null && <ChannelPicker onPick={pick} onClose={() => setPicking(null)} />}
    </div>
  );
}
