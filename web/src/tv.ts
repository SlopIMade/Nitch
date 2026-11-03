// TV support (M5): spatial navigation with a remote, TV mode, and the Back key.
//
// Instead of wrapping every component in a focus library, arrows move the browser focus
// geometrically: to the nearest focusable element in that direction. Every button, link and
// input works with the remote without extra code. Components that use the arrows themselves
// (multiview focus, the grid editor) claim them while they need them.

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

/** Elements inside the topmost layer (a modal or the full-screen player), else the whole page. */
function candidates(): HTMLElement[] {
  const layer =
    document.querySelector(".modal") ?? document.querySelector(".player--full") ?? document.body;
  return [...layer.querySelectorAll<HTMLElement>(FOCUSABLE)].filter(visible);
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
  const ax = a.left + a.width / 2;
  const ay = a.top + a.height / 2;
  let best: HTMLElement | null = null;
  let bestScore = Infinity;
  for (const el of all) {
    if (el === current) continue;
    const b = el.getBoundingClientRect();
    const bx = b.left + b.width / 2;
    const by = b.top + b.height / 2;
    const dx = bx - ax;
    const dy = by - ay;
    // Only elements that lie in the direction, judged by their edges (not just centers).
    const inDir =
      dir === "right" ? b.left >= a.right - 4 : dir === "left" ? b.right <= a.left + 4 : dir === "down" ? b.top >= a.bottom - 4 : b.bottom <= a.top + 4;
    if (!inDir) continue;
    // Distance along the direction plus a penalty for drifting sideways.
    const main = dir === "left" || dir === "right" ? Math.abs(dx) : Math.abs(dy);
    const side = dir === "left" || dir === "right" ? Math.abs(dy) : Math.abs(dx);
    const score = main + side * 2;
    if (score < bestScore) {
      bestScore = score;
      best = el;
    }
  }
  if (!best) return false;
  best.focus();
  best.scrollIntoView({ block: "nearest", inline: "nearest", behavior: "smooth" });
  return true;
}

const KEYS: Record<string, Dir> = { ArrowUp: "up", ArrowDown: "down", ArrowLeft: "left", ArrowRight: "right" };

/** Back: the topmost thing closes first; at the start page the TV app exits. */
export function installBackHandler(onBack: () => boolean) {
  const back = () => {
    if (onBack()) return;
    if (window.history.length > 1 && location.pathname !== "/") window.history.back();
    else window.NitchTV?.exit();
  };
  window.addEventListener("nitch:back", back);
  return () => window.removeEventListener("nitch:back", back);
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
