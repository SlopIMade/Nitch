import { GRID, type Rect } from "../api";

/** Tiny drawing of a multiview layout on the 16:9 canvas. */
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
