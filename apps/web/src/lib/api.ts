import { ApiError } from "@/lib/http";
import { USING_MOCKS } from "@/lib/flags";
import { mockApi } from "@/lib/mock";
import type {
  AuthResult,
  GameDetail,
  GameList,
  GameListQuery,
  GamePatch,
  GameSummary,
  IngestJob,
  IngestStats,
  LibraryFile,
  LibraryStats,
  NewsPost,
  ProgressState,
  Role,
  SaveSnapshot,
  ScanStatus,
  ServerConfig,
  SetupStatus,
  User,
} from "@/lib/types";

/* --------------------------------------------------------------------------
   One API surface for both adapters. `USING_MOCKS` is true in dev unless
   VITE_USE_MOCKS=false, and false in production unless it is set to true.
   -------------------------------------------------------------------------- */

export { USING_MOCKS };

export const TOKEN_KEY = "ludex.token";

let baseUrl = ((import.meta.env.VITE_API_URL as string | undefined) ?? "").replace(/\/$/, "");

export function setApiBase(url: string | null | undefined) {
  baseUrl = (url ?? "").replace(/\/$/, "");
}

export function getApiBase(): string {
  return baseUrl;
}

/**
 * Media and file URLs come back from the server as origin-relative paths
 * (`/api/media/...`). In Tauri the page origin is `tauri://localhost`, so a
 * bare relative URL would resolve against the webview, not the server. Every
 * image, background, video and download link must pass through this helper.
 */
export function assetUrl(path: string | null | undefined): string | null {
  if (!path) return null;
  if (/^(https?:|data:|blob:|demo:)/i.test(path)) return path;
  return `${baseUrl}${path.startsWith("/") ? "" : "/"}${path}`;
}

export const tokenStore = {
  get(): string | null {
    return localStorage.getItem(TOKEN_KEY);
  },
  set(token: string) {
    localStorage.setItem(TOKEN_KEY, token);
  },
  clear() {
    localStorage.removeItem(TOKEN_KEY);
  },
};

export const UNAUTHORIZED_EVENT = "ludex:unauthorized";

type RequestOptions = {
  method?: "GET" | "POST" | "PUT" | "PATCH" | "DELETE";
  body?: unknown;
  query?: Record<string, string | number | boolean | undefined | null>;
  auth?: boolean;
};

function buildUrl(path: string, query?: RequestOptions["query"]): string {
  const url = `${baseUrl}/api${path}`;
  if (!query) return url;
  const params = new URLSearchParams();
  for (const [key, value] of Object.entries(query)) {
    if (value === undefined || value === null || value === "") continue;
    params.set(key, String(value));
  }
  const qs = params.toString();
  return qs ? `${url}?${qs}` : url;
}

async function request<T>(path: string, options: RequestOptions = {}): Promise<T> {
  const headers: Record<string, string> = { Accept: "application/json" };
  const token = tokenStore.get();
  if (options.auth !== false && token) headers.Authorization = `Bearer ${token}`;
  if (options.body !== undefined) headers["Content-Type"] = "application/json";

  let response: Response;
  try {
    response = await fetch(buildUrl(path, options.query), {
      method: options.method ?? "GET",
      headers,
      body: options.body === undefined ? undefined : JSON.stringify(options.body),
    });
  } catch {
    throw new ApiError(0, "network_error", "Cannot reach the server. Check that it is running and the address is correct.");
  }

  if (response.status === 401) {
    tokenStore.clear();
    window.dispatchEvent(new CustomEvent(UNAUTHORIZED_EVENT));
  }

  if (response.status === 204) return undefined as T;

  const text = await response.text();
  let payload: unknown = null;
  if (text) {
    try {
      payload = JSON.parse(text);
    } catch {
      payload = null;
    }
  }

  if (!response.ok) {
    const envelope = payload as { error?: { code?: string; message?: string } } | null;
    throw new ApiError(
      response.status,
      envelope?.error?.code ?? "request_failed",
      envelope?.error?.message ?? `Request failed with status ${response.status}`,
    );
  }

  return payload as T;
}

const query = (q: GameListQuery): Record<string, string | number | boolean | undefined> => ({
  q: q.q,
  genre: q.genre,
  state: q.state,
  bookmarked: q.bookmarked,
  earlyAccess: q.earlyAccess,
  sort: q.sort,
  order: q.order,
  page: q.page,
  limit: q.limit,
});

export const api = {
  /* public */
  setupStatus(): Promise<SetupStatus> {
    return USING_MOCKS ? mockApi.setupStatus() : request<SetupStatus>("/setup/status", { auth: false });
  },
  setup(body: { username: string; password: string; serverName?: string }): Promise<AuthResult> {
    return USING_MOCKS ? mockApi.setup(body) : request<AuthResult>("/setup", { method: "POST", body, auth: false });
  },
  health(): Promise<{ status: string; version: string; uptimeSeconds: number }> {
    return USING_MOCKS ? mockApi.health() : request<{ status: string; version: string; uptimeSeconds: number }>("/health", { auth: false });
  },
  config(): Promise<ServerConfig> {
    return USING_MOCKS ? mockApi.config() : request<ServerConfig>("/config", { auth: false });
  },

  /* auth */
  login(body: { username: string; password: string }): Promise<AuthResult> {
    return USING_MOCKS ? mockApi.login(body) : request<AuthResult>("/auth/login", { method: "POST", body, auth: false });
  },
  me(): Promise<User> {
    if (USING_MOCKS) return mockApi.me(tokenStore.get());
    return request<{ user: User }>("/auth/me").then((r) => r.user);
  },
  logout(): Promise<void> {
    if (USING_MOCKS) return Promise.resolve();
    return request("/auth/logout", { method: "POST" });
  },
  oidcExchange(code: string): Promise<AuthResult> {
    return USING_MOCKS
      ? mockApi.oidcExchange(code)
      : request<AuthResult>("/auth/oidc/exchange", { method: "POST", body: { code }, auth: false });
  },
  changePassword(body: { currentPassword: string; newPassword: string }): Promise<void> {
    return USING_MOCKS
      ? mockApi.changePassword(tokenStore.get(), body)
      : request("/auth/change-password", { method: "POST", body });
  },

  /* games */
  listGames(q: GameListQuery = {}): Promise<GameList> {
    return USING_MOCKS ? mockApi.listGames(q) : request<GameList>("/games", { query: query(q) });
  },
  getGame(id: number): Promise<GameDetail> {
    return USING_MOCKS ? mockApi.getGame(id) : request<GameDetail>(`/games/${id}`);
  },
  randomGame(q: GameListQuery = {}): Promise<GameSummary> {
    return USING_MOCKS ? mockApi.randomGame(q) : request<GameSummary>("/games/random", { query: query(q) });
  },
  patchGame(id: number, patch: GamePatch): Promise<GameDetail> {
    return USING_MOCKS
      ? mockApi.patchGame(tokenStore.get(), id, patch)
      : request<GameDetail>(`/games/${id}`, { method: "PATCH", body: patch });
  },
  refreshMetadata(id: number): Promise<{ queued: boolean }> {
    return USING_MOCKS
      ? mockApi.refreshMetadata(tokenStore.get(), id)
      : request<{ queued: boolean }>(`/games/${id}/refresh-metadata`, { method: "POST" });
  },
  downloadTicket(id: number, versionId?: number): Promise<{ url: string; expiresAt: string }> {
    if (USING_MOCKS) return Promise.resolve({ url: "", expiresAt: new Date(Date.now() + 15 * 60_000).toISOString() });
    return request<{ url: string; expiresAt: string }>(`/games/${id}/download-ticket`, { method: "POST", body: { versionId } });
  },
  setProgress(id: number, state: ProgressState): Promise<{ state: ProgressState; minutesPlayed: number; lastPlayedAt: string | null }> {
    return USING_MOCKS
      ? mockApi.setProgress(tokenStore.get(), id, state)
      : request<{ state: ProgressState; minutesPlayed: number; lastPlayedAt: string | null }>(`/games/${id}/progress`, { method: "PUT", body: { state } });
  },
  addPlaytime(id: number, minutes: number): Promise<{ state: ProgressState; minutesPlayed: number; lastPlayedAt: string | null }> {
    return USING_MOCKS
      ? mockApi.addPlaytime(tokenStore.get(), id, minutes)
      : request<{ state: ProgressState; minutesPlayed: number; lastPlayedAt: string | null }>(`/games/${id}/playtime`, { method: "POST", body: { minutes } });
  },
  setBookmark(id: number, bookmarked: boolean): Promise<{ bookmarked: boolean }> {
    return USING_MOCKS
      ? mockApi.setBookmark(tokenStore.get(), id, bookmarked)
      : request<{ bookmarked: boolean }>(`/games/${id}/bookmark`, { method: "PUT", body: { bookmarked } });
  },
  stats(): Promise<LibraryStats> {
    return USING_MOCKS ? mockApi.stats(tokenStore.get()) : request<LibraryStats>("/stats");
  },

  /* users */
  listUsers(): Promise<User[]> {
    return USING_MOCKS ? mockApi.listUsers(tokenStore.get()) : request<User[]>("/users");
  },
  createUser(body: { username: string; password: string; email?: string | null; role: Role }): Promise<User> {
    return USING_MOCKS
      ? mockApi.createUser(tokenStore.get(), body)
      : request<User>("/users", { method: "POST", body });
  },
  updateUser(id: number, patch: { email?: string | null; role?: Role; activated?: boolean }): Promise<User> {
    return USING_MOCKS
      ? mockApi.updateUser(tokenStore.get(), id, patch)
      : request<User>(`/users/${id}`, { method: "PATCH", body: patch });
  },
  deleteUser(id: number): Promise<void> {
    return USING_MOCKS ? mockApi.deleteUser(tokenStore.get(), id) : request(`/users/${id}`, { method: "DELETE" });
  },

  /* library */
  scanStatus(): Promise<ScanStatus> {
    return USING_MOCKS ? mockApi.scanStatus(tokenStore.get()) : request<ScanStatus>("/library/scan");
  },
  startScan(): Promise<{ started: boolean }> {
    return USING_MOCKS ? mockApi.startScan(tokenStore.get()) : request<{ started: boolean }>("/library/scan", { method: "POST" });
  },
  folders(): Promise<{ gamesDir: string; mediaDir: string; dataDir: string }> {
    return USING_MOCKS ? mockApi.folders(tokenStore.get()) : request<{ gamesDir: string; mediaDir: string; dataDir: string }>("/library/folders");
  },

  /* ingest */
  listJobs(): Promise<IngestJob[]> {
    return USING_MOCKS ? mockApi.listJobs(tokenStore.get()) : request<IngestJob[]>("/ingest/downloads");
  },
  createJob(body: { url: string; title?: string }): Promise<IngestJob> {
    return USING_MOCKS
      ? mockApi.createJob(tokenStore.get(), body)
      : request<IngestJob>("/ingest/downloads", { method: "POST", body });
  },
  getJob(id: string): Promise<IngestJob> {
    return USING_MOCKS ? mockApi.getJob(tokenStore.get(), id) : request<IngestJob>(`/ingest/downloads/${id}`);
  },
  cancelJob(id: string): Promise<void> {
    return USING_MOCKS
      ? mockApi.cancelJob(tokenStore.get(), id)
      : request(`/ingest/downloads/${id}`, { method: "DELETE" });
  },
  retryJob(id: string): Promise<IngestJob> {
    return USING_MOCKS
      ? mockApi.retryJob(tokenStore.get(), id)
      : request<IngestJob>(`/ingest/downloads/${id}/retry`, { method: "POST" });
  },
  replaceJobUrl(id: string, url: string): Promise<IngestJob> {
    return USING_MOCKS
      ? mockApi.replaceJobUrl(tokenStore.get(), id, url)
      : request<IngestJob>(`/ingest/downloads/${id}`, { method: "PUT", body: { url } });
  },
  listFiles(): Promise<LibraryFile[]> {
    return USING_MOCKS ? mockApi.listFiles(tokenStore.get()) : request<LibraryFile[]>("/ingest/files");
  },
  deleteFile(name: string): Promise<void> {
    return USING_MOCKS
      ? mockApi.deleteFile(tokenStore.get(), name)
      : request(`/ingest/files/${encodeURIComponent(name)}`, { method: "DELETE" });
  },
  verifyFile(name: string): Promise<{ ok: boolean; log: string }> {
    return USING_MOCKS
      ? mockApi.verifyFile(tokenStore.get(), name)
      : request<{ ok: boolean; log: string }>(`/ingest/files/${encodeURIComponent(name)}/verify`, { method: "POST" });
  },
  ingestStats(): Promise<IngestStats> {
    return USING_MOCKS ? mockApi.ingestStats(tokenStore.get()) : request<IngestStats>("/ingest/stats");
  },

  /* cloud saves */
  listSaves(gameId: number): Promise<SaveSnapshot[]> {
    return USING_MOCKS
      ? mockApi.listSaves(tokenStore.get(), gameId)
      : request(`/games/${gameId}/saves`);
  },

  uploadSave(gameId: number, file: File, label?: string, onProgress?: (loaded: number, total: number) => void): Promise<SaveSnapshot> {
    if (USING_MOCKS) return mockApi.uploadSave(tokenStore.get(), gameId, file, label);
    return uploadWithProgress<SaveSnapshot>(`/games/${gameId}/saves`, file, label, onProgress);
  },

  deleteSave(id: number): Promise<void> {
    return USING_MOCKS
      ? mockApi.deleteSave(tokenStore.get(), id)
      : request(`/saves/${id}`, { method: "DELETE" });
  },

  async downloadSave(id: number): Promise<void> {
    if (USING_MOCKS) {
      await mockApi.downloadSave(tokenStore.get(), id);
      return;
    }
    const token = tokenStore.get();
    let response: Response;
    try {
      response = await fetch(buildUrl(`/saves/${id}/download`), {
        headers: token ? { Authorization: `Bearer ${token}` } : undefined,
      });
    } catch {
      throw new ApiError(0, "network_error", "Cannot reach the server. Check that it is running.");
    }

    if (response.status === 401) {
      tokenStore.clear();
      window.dispatchEvent(new CustomEvent(UNAUTHORIZED_EVENT));
    }

    if (!response.ok) {
      throw new ApiError(response.status, "download_failed", saveErrorMessage(response.status));
    }

    const blob = await response.blob();
    const disposition = response.headers.get("Content-Disposition") ?? "";
    const filename = filenameFromDisposition(disposition) ?? `ludex-save-${id}.zip`;
    const url = URL.createObjectURL(blob);
    const anchor = document.createElement("a");
    anchor.href = url;
    anchor.download = filename;
    anchor.rel = "noopener";
    anchor.click();
    setTimeout(() => URL.revokeObjectURL(url), 30_000);
  },

  /* news */
  listNews(): Promise<NewsPost[]> {
    return USING_MOCKS ? mockApi.listNews() : request<NewsPost[]>("/news");
  },
  createNews(body: { title: string; body: string }): Promise<NewsPost> {
    return USING_MOCKS
      ? mockApi.createNews(tokenStore.get(), body)
      : request<NewsPost>("/news", { method: "POST", body });
  },
  updateNews(id: number, body: { title?: string; body?: string }): Promise<NewsPost> {
    return USING_MOCKS
      ? mockApi.updateNews(tokenStore.get(), id, body)
      : request<NewsPost>(`/news/${id}`, { method: "PATCH", body });
  },
  deleteNews(id: number): Promise<void> {
    return USING_MOCKS ? mockApi.deleteNews(tokenStore.get(), id) : request(`/news/${id}`, { method: "DELETE" });
  },
};

export { ApiError };

/* ------------------------------ cloud saves ------------------------------ */

function saveErrorMessage(status: number): string {
  if (status === 401) return "Your session expired. Sign in again.";
  if (status === 404) return "That save no longer exists.";
  if (status === 413) return "That save is larger than the server allows (2 GiB).";
  return `The server refused the request (${status}).`;
}

function filenameFromDisposition(disposition: string): string | null {
  const utf8 = /filename\*=UTF-8''([^;]+)/i.exec(disposition);
  if (utf8?.[1]) {
    try {
      return decodeURIComponent(utf8[1]);
    } catch {
      return utf8[1];
    }
  }
  const plain = /filename="?([^";]+)"?/i.exec(disposition);
  return plain?.[1] ?? null;
}

/**
 * XHR instead of fetch: upload progress is only reliably available through
 * `xhr.upload.onprogress`, and saves can be large. Bearer auth is set by hand
 * because FormData must not set Content-Type.
 */
function uploadWithProgress<T>(
  path: string,
  file: File,
  label: string | undefined,
  onProgress?: (loaded: number, total: number) => void,
): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    const xhr = new XMLHttpRequest();
    xhr.open("POST", buildUrl(path));
    const token = tokenStore.get();
    if (token) xhr.setRequestHeader("Authorization", `Bearer ${token}`);

    xhr.upload.onprogress = (event) => {
      if (event.lengthComputable) onProgress?.(event.loaded, event.total);
    };

    xhr.onerror = () => reject(new ApiError(0, "network_error", "Cannot reach the server. Check that it is running."));
    xhr.onload = () => {
      let payload: unknown = null;
      if (xhr.responseText) {
        try {
          payload = JSON.parse(xhr.responseText);
        } catch {
          payload = null;
        }
      }

      if (xhr.status === 401) {
        tokenStore.clear();
        window.dispatchEvent(new CustomEvent(UNAUTHORIZED_EVENT));
      }

      if (xhr.status >= 200 && xhr.status < 300) {
        resolve(payload as T);
        return;
      }

      const envelope = payload as { error?: { code?: string; message?: string } } | null;
      reject(
        new ApiError(
          xhr.status,
          envelope?.error?.code ?? "upload_failed",
          envelope?.error?.message ?? saveErrorMessage(xhr.status),
        ),
      );
    };

    const form = new FormData();
    form.append("file", file);
    if (label?.trim()) form.append("label", label.trim());
    xhr.send(form);
  });
}
