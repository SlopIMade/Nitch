// TV support (M5): spatial navigation with a remote, TV mode, and the Back key.
//
// Instead of wrapping every component in a focus library, arrows move the browser focus
// geometrically: to the nearest focusable element in that direction. Every button, link and
// input works with the remote without extra code. Components that use the arrows themselves
// (multiview focus, the grid editor) claim them while they need them.

import type { KeyboardEvent as ReactKeyboardEvent } from "react";

/** The Android TV shell sets this in its user agent and exposes `window.NitchTV`. */
export const isTv = /NitchTV/.test(navigator.userAgent);

type NitchTvBridge = { exit: () => void; setKeepScreenOn: (on: boolean) => void; changeServer?: () => void };
declare global {
  interface Window {
    NitchTV?: NitchTvBridge;
  }
}

type Claim = (e: KeyboardEvent) => boolean;
const claims = new Set<Claim>();

/**
 * While claimed, arrow keys are left to the claiming component. `when` narrows the claim (e.g.
 * only left/right, only while the focus isn't in the chat). Returns the release function.
 */
export function claimArrows(when: Claim = () => true): () => void {
  const claim: Claim = (e) => when(e);
  claims.add(claim);
  return () => {
    claims.delete(claim);
  };
}

/** True if the focus is inside one of `selectors` (e.g. the chat or the player's control bar). */
export function focusWithin(...selectors: string[]): boolean {
  const el = document.activeElement;
  return !!el && selectors.some((s) => el.closest(s));
}

const FOCUSABLE = "button:not(:disabled), a[href], input:not(:disabled), select:not(:disabled), [tabindex]:not([tabindex='-1'])";

function visible(el: Element): boolean {
  const r = el.getBoundingClientRect();
  if (r.width === 0 || r.height === 0) return false;
  const style = getComputedStyle(el);
  return style.visibility !== "hidden" && style.display !== "none";
}

/** Elements inside the topmost layer (a modal, the side menu, the player), else the whole page. */
function candidates(): HTMLElement[] {
  const layer =
    document.querySelector(".modal") ?? document.querySelector(".sidemenu") ?? document.querySelector(".player") ?? document.body;
  // tabindex=-1 marks things only the mouse uses (multiview tiles, the chat's gear).
  return [...layer.querySelectorAll<HTMLElement>(FOCUSABLE)].filter((el) => el.getAttribute("tabindex") !== "-1" && visible(el));
}

type Dir = "up" | "down" | "left" | "right";

/** Moves the focus to the best element in direction `dir`; returns false if there's none. */
export function moveFocus(dir: Dir): boolean {
  const all = candidates();
  const current = document.activeElement as HTMLElement | null;
  if (!current || current === document.body || !all.includes(current)) {
    all[0]?.focus();
    return all.length > 0;
  }
  const a = current.getBoundingClientRect();
  const vertical = dir === "up" || dir === "down";
  // Only elements that lie in the direction, judged by their edges (not just centers), with the
  // gap between the edges.
  const ahead = all.flatMap((el) => {
    if (el === current) return [];
    const b = el.getBoundingClientRect();
    const gap = dir === "down" ? b.top - a.bottom : dir === "up" ? a.top - b.bottom : dir === "right" ? b.left - a.right : a.left - b.right;
    return gap >= -4 ? [{ el, b, gap: Math.max(0, gap) }] : [];
  });
  // How far apart two boxes are across the direction (0 when they overlap).
  const apart = (b: DOMRect) =>
    vertical ? Math.max(0, b.left - a.right, a.left - b.right) : Math.max(0, b.top - a.bottom, a.top - b.bottom);
  const centerOff = (b: DOMRect) =>
    vertical ? Math.abs(b.left + b.width / 2 - (a.left + a.width / 2)) : Math.abs(b.top + b.height / 2 - (a.top + a.height / 2));
  let pool = ahead;
  if (vertical) {
    // Up and down go to the next row (the nearest edge), and there to the closest element:
    // a menu's next line, not a button further down that happens to be straight below.
    const nearest = Math.min(...ahead.map((c) => c.gap));
    pool = ahead.filter((c) => c.gap <= nearest + 8);
  } else {
    // Left and right stay in the row: at its end they go nowhere (instead of jumping into
    // another row).
    pool = ahead.filter((c) => apart(c.b) === 0);
  }
  let best: HTMLElement | null = null;
  let bestScore = Infinity;
  for (const c of pool) {
    const score = vertical ? apart(c.b) * 4 + centerOff(c.b) : c.gap + apart(c.b) * 2 + centerOff(c.b) * 0.5;
    if (score < bestScore) {
      bestScore = score;
      best = c.el;
    }
  }
  if (!best) return false;
  best.focus();
  best.scrollIntoView({ block: "nearest", inline: "nearest", behavior: "smooth" });
  return true;
}

/** How long a key must be held to count as a long press. */
export const LONG_PRESS_MS = 450;

/**
 * Repeats of a held key come every ~50 ms. A press with no keydown for this long is over, even if
 * its keyup never arrived (Android can swallow it after a long press): it must not block the key.
 */
const STALE_MS = 600;

let press: { key: string; target: EventTarget | null; fired: boolean; timer: number; last: number } | null = null;

/** The current press of `key`, unless it went stale. */
function current(key: string) {
  if (press?.key === key && performance.now() - press.last < STALE_MS) return press;
  if (press?.key === key) {
    clearTimeout(press.timer);
    press = null;
  }
  return null;
}

/**
 * Tells short from long presses of `key`. Call it from keydown and keyup handlers: `onLong`
 * runs once the key has been held for LONG_PRESS_MS; keyup returns "short" if that didn't
 * happen. Timed, not counted: Android's WebView repeats a held key irregularly and never sets
 * `repeat`. On keydown the default action is prevented, so a held OK doesn't also click.
 */
export function pressKind(e: KeyboardEvent | ReactKeyboardEvent, key: string, onLong?: () => void): "short" | null {
  if (e.key !== key) return null;
  if (e.type === "keydown") {
    e.preventDefault();
    const held = current(key);
    if (held) {
      // A repeat of the key that's already down.
      held.last = performance.now();
      return null;
    }
    const p = { key, target: e.target, fired: false, timer: 0, last: performance.now() };
    p.timer = window.setTimeout(() => {
      if (press !== p) return;
      p.fired = true;
      onLong?.();
    }, LONG_PRESS_MS);
    press = p;
    return null;
  }
  if (e.type === "keyup" && press?.key === key) {
    clearTimeout(press.timer);
    const short = !press.fired && press.target === e.target;
    press = null;
    return short ? "short" : null;
  }
  return null;
}

// After a long press, the remote keeps repeating the key until it's released. Those repeats
// belong to the press: they must not reach whatever has the focus once the long press acted
// (a closed menu would otherwise reopen from the button that got the focus).
window.addEventListener(
  "keydown",
  (e) => {
    const held = current(e.key);
    if (held?.fired) {
      held.last = performance.now();
      e.preventDefault();
      e.stopImmediatePropagation();
    }
  },
  true,
);

// A key released where no handler asked (e.g. the long press opened a menu that took over):
// forget the press after the handlers had their turn.
window.addEventListener("keyup", (e) => {
  const p = press;
  if (p?.key === e.key)
    setTimeout(() => {
      if (press === p) {
        clearTimeout(p.timer);
        press = null;
      }
    });
});

const KEYS: Record<string, Dir> = { ArrowUp: "up", ArrowDown: "down", ArrowLeft: "left", ArrowRight: "right" };

/** Back handlers, the most recently added first: it's what opened last (a menu over the player). */
const backHandlers: (() => boolean)[] = [];

window.addEventListener("nitch:back", () => {
  for (let i = backHandlers.length - 1; i >= 0; i--) if (backHandlers[i]()) return;
  // Nothing open: go back a page; at the start page the TV app exits.
  if (window.history.length > 1 && location.pathname !== "/") window.history.back();
  else window.NitchTV?.exit();
});

/**
 * Handles the remote's Back key while installed: return true if it did something (closed a
 * menu, left the player), false to let the next handler down try. Returns the remover.
 */
export function installBackHandler(onBack: () => boolean): () => void {
  backHandlers.push(onBack);
  return () => {
    const i = backHandlers.lastIndexOf(onBack);
    if (i >= 0) backHandlers.splice(i, 1);
  };
}

/** Spatial navigation for the whole app. Call once. */
export function installSpatialNavigation() {
  if (isTv) document.documentElement.classList.add("tv");
  window.addEventListener("keydown", (e) => {
    const dir = KEYS[e.key];
    if (!dir || e.defaultPrevented || [...claims].some((c) => c(e))) return;
    const el = document.activeElement as HTMLElement | null;
    // Inside text fields, left/right move the caret.
    if (el && el.tagName === "INPUT" && (dir === "left" || dir === "right")) return;
    if (el && el.tagName === "SELECT") return;
    if (moveFocus(dir)) e.preventDefault();
  });
}
