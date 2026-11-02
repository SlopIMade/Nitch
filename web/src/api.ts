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
    request<{ id: string; hls_url: string }>("/api/play", { method: "POST", body: JSON.stringify({ channel }) }),
};

export function formatViewers(n: number): string {
  return n >= 1000 ? `${(n / 1000).toFixed(n >= 10000 ? 0 : 1)}K` : String(n);
}
