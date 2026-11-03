// The multiview grid (D13/D17): tiles snap to the 60×60 cells. A tile is an area of any shape;
// its 16:9 picture sits centered inside (drawn here, so the result is visible while editing).
// Mouse: drag to move, drag the corner to resize (Shift keeps the picture's 16:9 shape).
// Keyboard and remote editing is up to the page using it (see `nudge`).

import { useRef, type PointerEvent as ReactPointerEvent } from "react";
import { GRID } from "../api";

export type EditorTile = { channel: string | null; x: number; y: number; w: number; h: number; z: number };

const MIN = 4;
const clamp = (v: number, lo: number, hi: number) => Math.max(lo, Math.min(hi, v));

/** Keeps a tile inside the grid with at least MIN cells per side. */
export function fit(t: EditorTile): EditorTile {
  const w = clamp(t.w, MIN, GRID);
  const h = clamp(t.h, MIN, GRID);
  return { ...t, w, h, x: clamp(t.x, 0, GRID - w), y: clamp(t.y, 0, GRID - h) };
}

/** Moves (or resizes) a tile by whole cells. */
export function nudge(t: EditorTile, how: "move" | "size", dx: number, dy: number): EditorTile {
  return fit(how === "move" ? { ...t, x: t.x + dx, y: t.y + dy } : { ...t, w: t.w + dx, h: t.h + dy });
}

/** Shrinks the area to exactly the picture (equal cells across and down are 16:9). */
export function snug(t: EditorTile): EditorTile {
  const s = Math.min(t.w, t.h);
  return fit({ ...t, x: t.x + Math.floor((t.w - s) / 2), y: t.y + Math.floor((t.h - s) / 2), w: s, h: s });
}

type Drag = { index: number; mode: "move" | "resize"; sx: number; sy: number; start: EditorTile };

export function LayoutCanvas({
  tiles,
  selected,
  onSelect,
  onChange,
  onChannel,
  className = "",
}: {
  tiles: EditorTile[];
  selected: number | null;
  onSelect: (i: number | null) => void;
  onChange: (tiles: EditorTile[]) => void;
  /** Clicking a tile's name (choose its channel); none: the name is just a label. */
  onChannel?: (i: number) => void;
  className?: string;
}) {
  const canvas = useRef<HTMLDivElement>(null);
  const drag = useRef<Drag | null>(null);
  const pct = (n: number) => `${(n / GRID) * 100}%`;

  const down = (e: ReactPointerEvent, index: number, mode: Drag["mode"]) => {
    e.stopPropagation();
    (e.target as HTMLElement).setPointerCapture(e.pointerId);
    onSelect(index);
    drag.current = { index, mode, sx: e.clientX, sy: e.clientY, start: tiles[index] };
  };
  const move = (e: ReactPointerEvent) => {
    const d = drag.current;
    const box = canvas.current?.getBoundingClientRect();
    if (!d || !box) return;
    const dx = Math.round(((e.clientX - d.sx) / box.width) * GRID);
    const dy = Math.round(((e.clientY - d.sy) / box.height) * GRID);
    const s = d.start;
    let next: EditorTile;
    if (d.mode === "move") next = { ...s, x: s.x + dx, y: s.y + dy };
    else if (e.shiftKey) {
      const n = Math.max(s.w + dx, s.h + dy);
      next = { ...s, w: n, h: n };
    } else next = { ...s, w: s.w + dx, h: s.h + dy };
    onChange(tiles.map((t, k) => (k === d.index ? fit(next) : t)));
  };

  return (
    <div
      className={`layout-canvas ${className}`}
      ref={canvas}
      onPointerDown={() => onSelect(null)}
      onPointerMove={move}
      onPointerUp={() => (drag.current = null)}
    >
      {tiles.map((t, i) => (
        <div
          key={i}
          className={`layout-tile${i === selected ? " layout-tile--selected" : ""}${t.channel ? "" : " layout-tile--empty"}`}
          style={{ left: pct(t.x), top: pct(t.y), width: pct(t.w), height: pct(t.h), zIndex: t.z + 1 }}
          onPointerDown={(e) => down(e, i, "move")}
        >
          <div className="layout-tile__picture" />
          {onChannel ? (
            <button className="layout-tile__name" onPointerDown={(e) => e.stopPropagation()} onClick={() => onChannel(i)} tabIndex={-1}>
              {t.channel ?? "+ Choose channel"}
            </button>
          ) : (
            <span className="layout-tile__name">{t.channel}</span>
          )}
          <span className="layout-tile__size">
            {t.w}×{t.h}
          </span>
          <div className="layout-tile__handle" onPointerDown={(e) => down(e, i, "resize")} title="Resize (Shift keeps 16:9)" />
        </div>
      ))}
    </div>
  );
}
