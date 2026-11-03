// Typed client for the Nitch server API (see docs/ARCHITECTURE.md → Server API).

export type User = {
  id: string;
  login: string;
  display_name: string;
  profile_image_url: string;
};

export type AuthStatus =
  | { state: "logged_out" }
  | { state: "pending"; user_code: string; verification_uri: string; expires_at: number }
  | { state: "logged_in"; user: User };

export type LiveStream = {
  login: string;
  display_name: string;
  title: string;
  game: string;
  viewers: number;
  started_at: string;
  thumbnail_url: string;
};

export type SearchResult = {
  login: string;
  display_name: string;
  title: string;
  game: string;
  is_live: boolean;
  avatar_url: string;
};

export type ChannelInfo = {
  login: string;
  display_name: string;
  avatar_url: string;
  live: LiveStream | null;
};

/** Cells per side of the multiview canvas (D17). Equal w and h is exactly 16:9. */
export const GRID = 60;

export type Tile = { channel: string; x: number; y: number; w: number; h: number; z: number; fit?: "contain" | "cover" };
export type Rect = { x: number; y: number; w: number; h: number };
export type Session = { id: string; name: string; tiles: Tile[]; created_at: number; updated_at: number };
export type SlotInfo = Tile & { state: "live" | "loading" | "ad" | "retrying" | "offline" };
/** Audio rendition `k` of the multiview stream belongs to `slots[k]`. */
export type MultiviewInfo = { hls_url: string; grid: number; slots: (SlotInfo | null)[] };

export type PlayStatus = {
  channel: string;
  /** Set while an ad break is in the playlist; `from`/`until` are stream time (Unix ms). */
  ad: { from: number; until: number | null; filled_by: "backup" | "slate" | "nothing" } | null;
};

/** Testing ad handling: `localStorage["nitch.debug.play"] = '{"simulate_ads":true,"backup":false}'`. */
function debugPlayOptions(): Record<string, unknown> {
  try {
    return JSON.parse(localStorage.getItem("nitch.debug.play") ?? "{}");
  } catch {
    return {};
  }
}

export class ApiError extends Error {
  status: number;
  code: string;

  constructor(status: number, code: string, message: string) {
    super(message);
    this.status = status;
    this.code = code;
  }
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(path, { ...init, headers: { "content-type": "application/json", ...init?.headers } });
  if (!res.ok) {
    let body: { error?: string; message?: string } = {};
    try {
      body = await res.json();
    } catch {
      // not JSON
    }
    throw new ApiError(res.status, body.error ?? "error", body.message ?? res.statusText);
  }
  return res.json() as Promise<T>;
}

export const api = {
  auth: () => request<AuthStatus>("/api/auth"),
  startLogin: () => request<AuthStatus>("/api/auth/device", { method: "POST" }),
  logout: () => request<AuthStatus>("/api/auth", { method: "DELETE" }),
  followsLive: () => request<LiveStream[]>("/api/follows/live"),
  search: (q: string) => request<SearchResult[]>(`/api/search?q=${encodeURIComponent(q)}`),
  channel: (login: string) => request<ChannelInfo>(`/api/channels/${encodeURIComponent(login)}`),
  play: (channel: string) =>
    request<{ id: string; hls_url: string }>("/api/play", {
      method: "POST",
      body: JSON.stringify({ ...debugPlayOptions(), channel }),
    }),
  playStatus: (id: string) => request<PlayStatus>(`/api/play/${encodeURIComponent(id)}`),
  /** The player left this stream; the server ends the play session right away. */
  stopPlay: (id: string) => fetch(`/api/play/${encodeURIComponent(id)}`, { method: "DELETE", keepalive: true }).catch(() => {}),

  sessions: () => request<Session[]>("/api/sessions"),
  session: (id: string) => request<Session>(`/api/sessions/${encodeURIComponent(id)}`),
  createSession: (name: string, tiles: Tile[]) =>
    request<Session>("/api/sessions", { method: "POST", body: JSON.stringify({ name, tiles }) }),
  updateSession: (id: string, patch: { name?: string; tiles?: Tile[] }) =>
    request<Session>(`/api/sessions/${encodeURIComponent(id)}`, { method: "PATCH", body: JSON.stringify(patch) }),
  deleteSession: async (id: string) => {
    const res = await fetch(`/api/sessions/${encodeURIComponent(id)}`, { method: "DELETE" });
    if (!res.ok && res.status !== 404) throw new ApiError(res.status, "error", res.statusText);
  },
  playSession: (id: string) => request<MultiviewInfo>(`/api/sessions/${encodeURIComponent(id)}/play`, { method: "POST" }),
  multiviewStatus: (id: string) => request<MultiviewInfo>(`/api/sessions/${encodeURIComponent(id)}/play`),
};

export function formatViewers(n: number): string {
  return n >= 1000 ? `${(n / 1000).toFixed(n >= 10000 ? 0 : 1)}K` : String(n);
}
