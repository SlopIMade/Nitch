// Editor for a new or saved multiview session (D13/D17) on the 60×60 grid. A running session is
// better edited in the player itself (its layout mode), where changes apply live.
// Mouse: drag tiles, drag the corner to resize (Shift keeps 16:9). Keyboard: arrows move the
// selected tile, Shift + arrows resize it, Delete removes it.

import { useEffect, useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useNavigate, useParams, useSearchParams } from "react-router";
import { api, type Tile } from "../api";
import { ChannelPicker } from "../components/ChannelPicker";
import { Icon } from "../components/Icon";
import { fit, LayoutCanvas, nudge, snug, type EditorTile } from "../components/LayoutCanvas";
import { usePlayer } from "../player/PlayerContext";
import { claimArrows, installBackHandler } from "../tv";

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

  const update = (i: number, f: (t: EditorTile) => EditorTile) => setTiles((ts) => ts && ts.map((t, k) => (k === i ? fit(f(t)) : t)));
  const remove = (i: number) => {
    setTiles((ts) => ts && ts.filter((_, k) => k !== i));
    setSelected(null);
  };
  const toFront = (i: number) => update(i, (t) => ({ ...t, z: Math.max(0, ...(tiles ?? []).map((o) => o.z)) + 1 }));

  // While a tile is selected, the arrows move it instead of the focus; Back lets go of it.
  useEffect(() => {
    if (selected === null || picking !== null) return;
    const release = claimArrows();
    const removeBack = installBackHandler(() => {
      setSelected(null);
      return true;
    });
    return () => {
      release();
      removeBack();
    };
  }, [selected, picking]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (selected === null || picking !== null || (e.target as HTMLElement).tagName === "INPUT") return;
      const d = { ArrowLeft: [-1, 0], ArrowRight: [1, 0], ArrowUp: [0, -1], ArrowDown: [0, 1] }[e.key];
      if (d) {
        e.preventDefault();
        update(selected, (t) => nudge(t, e.shiftKey ? "size" : "move", d[0], d[1]));
      } else if (e.key === "Delete" || e.key === "Backspace") {
        remove(selected);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

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
  const empty = tiles.length - complete.length;
  const sel = selected !== null ? tiles[selected] : null;

  return (
    <div className="page editor">
      <div className="toolbar">
        <input className="input" value={name} onChange={(e) => setName(e.target.value)} maxLength={80} aria-label="Session name" />
        <button className="btn btn--ghost" onClick={() => setPicking("new")} disabled={tiles.length >= 9}>
          <Icon name="plus" /> Add stream
        </button>
        <div className="toolbar__spacer" />
        {empty > 0 && <span className="muted">{empty} tile(s) without a channel won't be saved</span>}
        <button className="btn btn--ghost" disabled={save.isPending || complete.length === 0} onClick={() => save.mutate(false)}>
          Save
        </button>
        <button className="btn" disabled={save.isPending || complete.length === 0} onClick={() => save.mutate(true)}>
          <Icon name="play" /> Save & play
        </button>
      </div>
      {/* Actions for the selected tile live here, so they stay reachable when tiles overlap. */}
      <div className="toolbar toolbar--sub">
        {/* Picking a tile without a mouse (remote); the arrows then move it. */}
        <button
          className="btn btn--ghost btn--icon btn--small"
          onClick={() => tiles.length && setSelected(((selected ?? 0) - 1 + tiles.length) % tiles.length)}
          title="Previous tile"
        >
          <Icon name="prev" />
        </button>
        <button
          className="btn btn--ghost btn--icon btn--small"
          onClick={() => tiles.length && setSelected(selected === null ? 0 : (selected + 1) % tiles.length)}
          title="Next tile"
        >
          <Icon name="next" />
        </button>
        {sel && selected !== null ? (
          <>
            <strong>{sel.channel ?? "Empty tile"}</strong>
            <span className="muted">
              {sel.w}×{sel.h}
            </span>
            <button className="btn btn--ghost btn--small" onClick={() => setPicking(selected)}>
              Channel
            </button>
            <button className="btn btn--ghost btn--small" onClick={() => update(selected, snug)} title="Shrink the area to the 16:9 picture">
              Fit to 16:9
            </button>
            <button className="btn btn--ghost btn--small" onClick={() => toFront(selected)}>
              To front
            </button>
            <button className="btn btn--ghost btn--small" onClick={() => remove(selected)}>
              <Icon name="trash" /> Remove
            </button>
          </>
        ) : (
          <span className="muted">Select a tile to change it.</span>
        )}
      </div>
      {save.isError && <p className="error">Saving failed: {save.error.message}</p>}

      <LayoutCanvas
        className="editor__canvas"
        tiles={tiles}
        selected={selected}
        onSelect={setSelected}
        onChange={setTiles}
        onChannel={setPicking}
      />
      <p className="muted">
        A tile is an area of any shape; its 16:9 picture is centered inside. Drag to move, drag the corner to resize (Shift keeps
        16:9). Arrow keys move the selected tile, Shift + arrows resize it, Delete removes it.
      </p>
      {picking !== null && <ChannelPicker onPick={pick} onClose={() => setPicking(null)} />}
    </div>
  );
}
