// Editing a running multiview's layout inside the player: the tiles are dragged right over the
// picture, which keeps playing (with its audio). Every change is saved shortly after and the
// server applies it live; the picture follows after the player's delay.
//
// Remote: the bar picks a tile (‹ ›), "Move" or "Resize" hands the arrows to that tile until OK
// or Back, the other buttons act on it. Back leaves the layout mode.

import { useEffect, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { api, type Tile } from "../api";
import { Icon } from "../components/Icon";
import { fit, nudge, snug, type EditorTile } from "../components/LayoutCanvas";
import { claimArrows, installBackHandler } from "../tv";

export type ArrowMode = "move" | "size" | null;

/** The layout being edited, saved to the server as it changes. */
export function useLayoutEdit(sessionId: string | null, serverTiles: Tile[], active: boolean) {
  const qc = useQueryClient();
  const [tiles, setTiles] = useState<EditorTile[]>([]);
  const [selected, setSelected] = useState<number | null>(null);
  const timer = useRef(0);

  // Start from the server's layout; again when tiles were added or removed meanwhile (side menu).
  const channels = serverTiles.map((t) => t.channel).join();
  useEffect(() => {
    if (!active) return;
    setTiles(serverTiles.map((t) => ({ ...t })));
    setSelected((s) => (s !== null && s < serverTiles.length ? s : serverTiles.length ? 0 : null));
  }, [active, channels]);

  const change = (next: EditorTile[]) => {
    setTiles(next);
    if (!sessionId) return;
    clearTimeout(timer.current);
    timer.current = window.setTimeout(async () => {
      const saved = next.flatMap((t) => (t.channel ? [{ ...t, channel: t.channel, fit: "contain" as const }] : []));
      const s = await api.updateSession(sessionId, { tiles: saved });
      qc.setQueryData(["session", sessionId], s);
      qc.invalidateQueries({ queryKey: ["sessions"] });
    }, 400);
  };
  const update = (i: number, f: (t: EditorTile) => EditorTile) => change(tiles.map((t, k) => (k === i ? fit(f(t)) : t)));

  return { tiles, selected, setSelected, change, update };
}

export function LayoutBar({
  edit,
  onAddStream,
  onRemove,
  onDone,
}: {
  edit: ReturnType<typeof useLayoutEdit>;
  onAddStream: () => void;
  onRemove: (channel: string) => void;
  onDone: () => void;
}) {
  const { tiles, selected, setSelected, update } = edit;
  const [arrows, setArrows] = useState<ArrowMode>(null);
  const bar = useRef<HTMLDivElement>(null);
  const sel = selected !== null ? tiles[selected] : null;
  const step = (d: number) => tiles.length && setSelected(((selected ?? 0) + d + tiles.length) % tiles.length);

  // The picture makes room for the bar (it wraps to two rows on narrow screens).
  useEffect(() => {
    const el = bar.current;
    const player = el?.closest<HTMLElement>(".player");
    if (!el || !player) return;
    const ro = new ResizeObserver(() => player.style.setProperty("--edit-bar", `${el.offsetHeight}px`));
    ro.observe(el);
    return () => {
      ro.disconnect();
      player.style.removeProperty("--edit-bar");
    };
  }, []);

  // Start on "Move" (once there's a tile to move), so the remote can go right away.
  const started = useRef(false);
  useEffect(() => {
    if (started.current || !sel) return;
    started.current = true;
    bar.current?.querySelector<HTMLElement>("[data-start]")?.focus();
  }, [sel]);

  // Back leaves the arrow mode, else the layout mode.
  useEffect(
    () =>
      installBackHandler(() => {
        if (arrows) setArrows(null);
        else onDone();
        return true;
      }),
    [arrows, onDone],
  );

  // Arrow mode: the arrows move or resize the selected tile until OK or Escape.
  useEffect(() => {
    if (!arrows || selected === null) return;
    const release = claimArrows();
    const onKey = (e: KeyboardEvent) => {
      const d = { ArrowLeft: [-1, 0], ArrowRight: [1, 0], ArrowUp: [0, -1], ArrowDown: [0, 1] }[e.key];
      if (d) {
        e.preventDefault();
        e.stopPropagation();
        update(selected, (t) => nudge(t, arrows, d[0], d[1]));
      } else if (e.key === "Enter" || e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        setArrows(null);
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => {
      release();
      window.removeEventListener("keydown", onKey, true);
    };
  }, [arrows, selected, tiles]);

  const toFront = (i: number) => update(i, (t) => ({ ...t, z: Math.max(0, ...tiles.map((o) => o.z)) + 1 }));

  return (
    <div className="player__bar player__bar--bottom player__bar--edit" ref={bar}>
      <button className="btn btn--ghost btn--icon" onClick={() => step(-1)} title="Previous tile">
        <Icon name="prev" />
      </button>
      <span className="edit__tile">
        {sel ? (
          <>
            <strong>{sel.channel}</strong>
            <span className="muted">
              {sel.w}×{sel.h}
            </span>
          </>
        ) : (
          <span className="muted">No tile</span>
        )}
      </span>
      <button className="btn btn--ghost btn--icon" onClick={() => step(1)} title="Next tile">
        <Icon name="next" />
      </button>
      <span className="toolbar__sep" />
      <button className={`btn btn--ghost${arrows === "move" ? " btn--on" : ""}`} data-start onClick={() => setArrows("move")} disabled={!sel}>
        <Icon name="move" /> Move
      </button>
      <button className={`btn btn--ghost${arrows === "size" ? " btn--on" : ""}`} onClick={() => setArrows("size")} disabled={!sel}>
        <Icon name="resize" /> Resize
      </button>
      <button className="btn btn--ghost" onClick={() => selected !== null && update(selected, snug)} disabled={!sel} title="Shrink the area to the 16:9 picture">
        16:9
      </button>
      <button className="btn btn--ghost" onClick={() => selected !== null && toFront(selected)} disabled={!sel}>
        To front
      </button>
      <button className="btn btn--ghost" onClick={() => sel?.channel && onRemove(sel.channel)} disabled={!sel || tiles.length < 2}>
        <Icon name="trash" /> Remove
      </button>
      <button className="btn btn--ghost" onClick={onAddStream} disabled={tiles.length >= 9}>
        <Icon name="plus" /> Stream
      </button>
      <span className="toolbar__spacer" />
      {arrows ? (
        <span className="edit__hint">Arrows {arrows === "move" ? "move" : "resize"} the tile · OK when done</span>
      ) : (
        <span className="edit__hint">Drag tiles or their corner · Shift keeps 16:9</span>
      )}
      <button className="btn" onClick={onDone}>
        <Icon name="check" /> Done
      </button>
    </div>
  );
}
