// Automatic multiview layouts, for adding and removing streams while watching. Every rect is
// 16:9 (equal width and height in grid cells, D17).

import { GRID, type Rect, type Tile } from "./api";

const r = (x: number, y: number, w: number, h = w): Rect => ({ x, y, w, h });

/** The standard layout for `n` tiles, biggest first. */
export function autoRects(n: number): Rect[] {
  if (n <= 1) return [r(0, 0, 60)];
  if (n === 2) return [r(0, 15, 30), r(30, 15, 30)];
  if (n === 3) return [r(0, 10, 40), r(40, 10, 20), r(40, 30, 20)];
  if (n === 4) return [r(0, 0, 30), r(30, 0, 30), r(0, 30, 30), r(30, 30, 30)];
  if (n <= 6) return [r(0, 0, 40), r(40, 0, 20), r(40, 20, 20), r(40, 40, 20), r(20, 40, 20), r(0, 40, 20)].slice(0, n);
  return Array.from({ length: Math.min(n, 9) }, (_, i) => r((i % 3) * 20, Math.floor(i / 3) * 20, 20));
}

const overlaps = (a: Rect, b: Rect) => a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;

/** The largest free 16:9 spot of at least `min` cells, or null. */
function freeSpot(tiles: Rect[], min: number): Rect | null {
  for (let s = GRID; s >= min; s--)
    for (let y = 0; y + s <= GRID; y++)
      for (let x = 0; x + s <= GRID; x++) {
        const c = r(x, y, s);
        if (!tiles.some((t) => overlaps(t, c))) return c;
      }
  return null;
}

const same = (tiles: Rect[], rects: Rect[]) =>
  tiles.length === rects.length && rects.every((q) => tiles.some((t) => t.x === q.x && t.y === q.y && t.w === q.w && t.h === q.h));

/** Lays out `tiles` in the standard layout, the biggest tile first. */
function relayout(tiles: Tile[]): Tile[] {
  const rects = autoRects(tiles.length);
  return [...tiles].sort((a, b) => b.w * b.h - a.w * a.h).map((t, i) => ({ ...t, ...rects[i], z: i }));
}

/**
 * Adds `channel`: into the largest free spot if there's a reasonable one and the layout is
 * custom, otherwise everything moves to the standard layout for one more tile.
 */
export function addTile(tiles: Tile[], channel: string): Tile[] {
  const z = Math.max(-1, ...tiles.map((t) => t.z)) + 1;
  const spot = same(tiles, autoRects(tiles.length)) ? null : freeSpot(tiles, 20);
  if (spot) return [...tiles, { channel, ...spot, z }];
  return relayout([...tiles, { channel, ...r(0, 0, 1), z }]);
}

/** Removes the tile at `index`; a standard layout stays standard. */
export function removeTile(tiles: Tile[], index: number): Tile[] {
  const rest = tiles.filter((_, i) => i !== index);
  return same(tiles, autoRects(tiles.length)) ? relayout(rest) : rest;
}
