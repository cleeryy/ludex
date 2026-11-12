import { ApiError } from "@/lib/http";
import { USING_MOCKS } from "@/lib/flags";
import { activeStorageScope, legacyTokenKey } from "@/lib/server-storage";
import { mockApi } from "@/lib/mock";
import { t } from "@/lib/i18n";
import { CONTROLLER_PRESETS, presetProfile } from "@/lib/controller-profiles";
import type {
  AchievementScanReport,
  ActivityPage,
  AuthResult,
  AuthSession,
  Collection,
  CollectionGamesResponse,
  CollectionInput,
  CompleteUploadResponse,
  ControllerProfile,
  ControllerProfileEntry,
  ControllerProfilesResponse,
  DesktopDownload,
  DesktopHandoff,
  DesktopPlatform,
  Emulator,
  EmulatorDefault,
  EmulatorLaunch,
  GameAchievements,
  GameDetail,
  GameList,
  GameListQuery,
  GamePatch,
  GameSummary,
  GameVersionsResponse,
  DeleteVersionResponse,
  IngestJob,
  IngestStats,
  LibraryFile,
  LibraryStats,
  MetadataApplyInput,
  MetadataCandidates,
  MetadataProvider,
  MetadataProviderInput,
  NewsPost,
  OidcAdminResponse,
  OidcSettingsInput,
  OidcTestResult,
  ProfilePatch,
  PlatformInfo,
  ProgressState,
  ProviderTestResult,
  PublicProfile,
  PublicProfileResponse,
  Role,
  SaveSnapshot,
  ScanStatus,
  ServerConfig,
  SetupStatus,
  SuggestResponse,
  UploadPartResponse,
  UploadSession,
  User,
  UserAchievements,
} from "@/lib/types";

/* --------------------------------------------------------------------------
   One API surface for both adapters. `USING_MOCKS` is true in dev unless
   VITE_USE_MOCKS=false, and false in production unless it is set to true.
   -------------------------------------------------------------------------- */

export { USING_MOCKS };

export const TOKEN_KEY = legacyTokenKey;

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

/**
 * Tokens are scoped per saved server in the browser (#24) so switching servers
 * never reuses another server's session. The desktop app keeps its token in
 * `settings.json` (0600) and the profile copies in the OS keychain, so it keeps
 * using the legacy key here.
 */
export const tokenStore = {
  get(): string | null {
    const scoped = activeStorageScope();
    if (scoped) return localStorage.getItem(scoped.tokenKey) ?? localStorage.getItem(TOKEN_KEY);
    return localStorage.getItem(TOKEN_KEY);
  },
  set(token: string) {
    const scoped = activeStorageScope();
    localStorage.setItem(scoped ? scoped.tokenKey : TOKEN_KEY, token);
  },
  clear() {
    const scoped = activeStorageScope();
    if (scoped) localStorage.removeItem(scoped.tokenKey);
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
    throw new ApiError(0, "network_error", t("Cannot reach the server. Check that it is running and the address is correct."));
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
  yearFrom: q.yearFrom,
  yearTo: q.yearTo,
  players: q.players,
  playtimeMin: q.playtimeMin,
  playtimeMax: q.playtimeMax,
  minRating: q.minRating,
  provider: q.provider,
  platform: q.platform,
  kind: q.kind,
  sort: q.sort,
  order: q.order,
  page: q.page,
  limit: q.limit,
});

export const api = {
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
  /**
   * Public desktop download metadata (#164): installer links for the requested
   * platform (or every asset), the release page and checksums. Never carries a
   * token so the page works logged out.
   */
  desktopDownload(platform?: DesktopPlatform): Promise<DesktopDownload> {
    return USING_MOCKS
      ? mockApi.desktopDownload(platform)
      : request<DesktopDownload>("/desktop/download", { auth: false, query: { platform } });
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
  /**
   * Desktop handoff (#157): binds the current web session to the state the
   * desktop app generated. Returns only the expiry; the desktop claims the
   * one-time code itself.
   */
  createDesktopHandoff(state: string): Promise<DesktopHandoff> {
    return USING_MOCKS
      ? mockApi.createDesktopHandoff(tokenStore.get(), state)
      : request<DesktopHandoff>("/auth/desktop-handoff", { method: "POST", body: { state } });
  },
  changePassword(body: { currentPassword: string; newPassword: string }): Promise<void> {
    return USING_MOCKS
      ? mockApi.changePassword(tokenStore.get(), body)
      : request("/auth/change-password", { method: "POST", body });
  },
  /** Self-service wizard preference (#119); returns the refreshed account. */
  updateOnboarding(body: { completed?: boolean; step?: number }): Promise<User> {
    return USING_MOCKS
      ? mockApi.updateOnboarding(tokenStore.get(), body)
      : request<{ user: User }>("/auth/onboarding", { method: "PUT", body }).then((r) => r.user);
  },

  /* admin OIDC (#103) */
  adminOidc(): Promise<OidcAdminResponse> {
    return USING_MOCKS ? mockApi.adminOidc(tokenStore.get()) : request<OidcAdminResponse>("/admin/oidc");
  },
  saveOidcSettings(body: OidcSettingsInput): Promise<OidcAdminResponse> {
    return USING_MOCKS
      ? mockApi.saveOidcSettings(tokenStore.get(), body)
      : request<OidcAdminResponse>("/admin/oidc", { method: "PUT", body });
  },
  testOidcSettings(body: OidcSettingsInput): Promise<OidcTestResult> {
    return USING_MOCKS
      ? mockApi.testOidcSettings(tokenStore.get(), body)
      : request<OidcTestResult>("/admin/oidc/test", { method: "POST", body });
  },
  resetOidcSettings(): Promise<OidcAdminResponse> {
    return USING_MOCKS
      ? mockApi.resetOidcSettings(tokenStore.get())
      : request<OidcAdminResponse>("/admin/oidc/reset", { method: "POST" });
  },

  /* admin metadata providers (#28) */
  adminMetadataProviders(): Promise<MetadataProvider[]> {
    return USING_MOCKS
      ? mockApi.adminMetadataProviders(tokenStore.get())
      : request<MetadataProvider[]>("/admin/metadata-providers");
  },
  createMetadataProvider(body: MetadataProviderInput): Promise<MetadataProvider> {
    return USING_MOCKS
      ? mockApi.createMetadataProvider(tokenStore.get(), body)
      : request<MetadataProvider>("/admin/metadata-providers", { method: "POST", body });
  },
  updateMetadataProvider(id: number, body: MetadataProviderInput): Promise<MetadataProvider> {
    return USING_MOCKS
      ? mockApi.updateMetadataProvider(tokenStore.get(), id, body)
      : request<MetadataProvider>(`/admin/metadata-providers/${id}`, { method: "PUT", body });
  },
  deleteMetadataProvider(id: number): Promise<void> {
    return USING_MOCKS
      ? mockApi.deleteMetadataProvider(tokenStore.get(), id)
      : request(`/admin/metadata-providers/${id}`, { method: "DELETE" });
  },
  reorderMetadataProviders(ids: number[]): Promise<MetadataProvider[]> {
    return USING_MOCKS
      ? mockApi.reorderMetadataProviders(tokenStore.get(), ids)
      : request<MetadataProvider[]>("/admin/metadata-providers/reorder", { method: "POST", body: { ids } });
  },
  testMetadataProvider(id: number, body: { title: string; year?: number }): Promise<ProviderTestResult> {
    return USING_MOCKS
      ? mockApi.testMetadataProvider(tokenStore.get(), id, body)
      : request<ProviderTestResult>(`/admin/metadata-providers/${id}/test`, { method: "POST", body });
  },

  /* games */
  listGames(q: GameListQuery = {}): Promise<GameList> {
    return USING_MOCKS ? mockApi.listGames(q) : request<GameList>("/games", { query: query(q) });
  },
  suggestGames(q: string, limit = 8): Promise<SuggestResponse> {
    return USING_MOCKS
      ? mockApi.suggestGames(q, limit)
      : request<SuggestResponse>("/games/suggest", { query: { q, limit } });
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
  /** Every version of a game, including missing rows (editor, #168). */
  gameVersions(id: number): Promise<GameVersionsResponse> {
    return USING_MOCKS
      ? mockApi.gameVersions(tokenStore.get(), id)
      : request<GameVersionsResponse>(`/games/${id}/versions`);
  },
  /** Pins the version used by downloads, installs and update checks (#168). */
  setDefaultGameVersion(id: number, versionId: number): Promise<GameVersionsResponse> {
    return USING_MOCKS
      ? mockApi.setDefaultGameVersion(tokenStore.get(), id, versionId)
      : request<GameVersionsResponse>(`/games/${id}/versions/default`, {
          method: "PUT",
          body: { versionId },
        });
  },
  /** Deletes one archive (not the game) after explicit confirmation (#168). */
  deleteGameVersion(id: number, versionId: number): Promise<DeleteVersionResponse> {
    return USING_MOCKS
      ? mockApi.deleteGameVersion(tokenStore.get(), id, versionId)
      : request<DeleteVersionResponse>(`/games/${id}/versions/${versionId}`, {
          method: "DELETE",
          query: { confirm: true },
        });
  },
  /**
   * Manual metadata match (#135): searches the configured providers. Runs
   * only when the editor opens the dialog, never on page load.
   */
  metadataCandidates(
    id: number,
    params: { q?: string; provider?: string; year?: number } = {},
  ): Promise<MetadataCandidates> {
    return USING_MOCKS
      ? mockApi.metadataCandidates(tokenStore.get(), id, params)
      : request<MetadataCandidates>(`/games/${id}/metadata/candidates`, {
          query: { q: params.q, provider: params.provider, year: params.year },
        });
  },
  /** Applies one provider record; returns the updated game detail. */
  applyMetadata(id: number, body: MetadataApplyInput): Promise<GameDetail> {
    return USING_MOCKS
      ? mockApi.applyMetadata(tokenStore.get(), id, body)
      : request<GameDetail>(`/games/${id}/metadata/apply`, { method: "POST", body });
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
  updateUser(
    id: number,
    patch: {
      username?: string;
      email?: string | null;
      password?: string;
      role?: Role;
      activated?: boolean;
      ageLimit?: number | null;
    },
  ): Promise<User> {
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

  /* controller profiles (#51) */
  controllerProfiles(): Promise<ControllerProfilesResponse> {
    return USING_MOCKS
      ? Promise.resolve({
          global: null,
          games: [],
          presets: CONTROLLER_PRESETS.map((id) => ({ id, label: presetProfile(id).name, profile: presetProfile(id) })),
        })
      : request<ControllerProfilesResponse>("/controller-profiles");
  },
  saveControllerProfile(gameId: number | null, profile: ControllerProfile): Promise<ControllerProfileEntry> {
    return USING_MOCKS
      ? Promise.resolve({ gameId, gameTitle: null, profile, updatedAt: new Date().toISOString() })
      : request<ControllerProfileEntry>("/controller-profiles", { method: "PUT", body: { gameId, profile } });
  },
  deleteControllerProfile(gameId: number): Promise<void> {
    return USING_MOCKS
      ? Promise.resolve()
      : request<void>(`/controller-profiles/${gameId}`, { method: "DELETE" });
  },

  /* emulators and ROMs (#52) */
  listPlatforms(): Promise<PlatformInfo[]> {
    return USING_MOCKS
      ? Promise.resolve([
          {
            id: "snes",
            label: "Super Nintendo",
            extensions: ["sfc", "smc"],
            retroarchCore: "snes9x",
            gameCount: 1,
          },
        ])
      : request<PlatformInfo[]>("/platforms");
  },
  listEmulators(): Promise<Emulator[]> {
    return USING_MOCKS ? Promise.resolve([]) : request<Emulator[]>("/emulators");
  },
  createEmulator(body: { name: string; command: string; platforms: string[]; enabled?: boolean }): Promise<Emulator> {
    return USING_MOCKS
      ? Promise.resolve({ id: 1, name: body.name, command: body.command, platforms: body.platforms, enabled: true, createdAt: "", updatedAt: "" })
      : request<Emulator>("/emulators", { method: "POST", body });
  },
  updateEmulator(id: number, patch: { name?: string; command?: string; platforms?: string[]; enabled?: boolean }): Promise<Emulator> {
    return USING_MOCKS
      ? Promise.resolve({ id, name: "", command: "", platforms: [], enabled: true, createdAt: "", updatedAt: "" })
      : request<Emulator>(`/emulators/${id}`, { method: "PATCH", body: patch });
  },
  deleteEmulator(id: number): Promise<void> {
    return USING_MOCKS ? Promise.resolve() : request<void>(`/emulators/${id}`, { method: "DELETE" });
  },
  validateEmulatorCommand(command: string): Promise<{ valid: boolean; error: string | null; program: string | null; args: string[] }> {
    return USING_MOCKS
      ? Promise.resolve({ valid: command.includes("%ROM%"), error: command.includes("%ROM%") ? null : "Missing %ROM%", program: null, args: [] })
      : request("/emulators/validate", { method: "POST", body: { command } });
  },
  emulatorDefaults(): Promise<EmulatorDefault[]> {
    return USING_MOCKS ? Promise.resolve([]) : request<EmulatorDefault[]>("/emulators/defaults");
  },
  setEmulatorDefaults(defaults: EmulatorDefault[]): Promise<EmulatorDefault[]> {
    return USING_MOCKS ? Promise.resolve(defaults) : request<EmulatorDefault[]>("/emulators/defaults", { method: "PUT", body: { defaults } });
  },
  setGameEmulator(id: number, emulatorId: number | null): Promise<void> {
    return USING_MOCKS
      ? Promise.resolve()
      : request<void>(`/games/${id}/emulator`, { method: "PUT", body: { emulatorId } });
  },
  launchCommand(id: number): Promise<EmulatorLaunch> {
    return USING_MOCKS
      ? Promise.reject(new Error("not available in demo mode"))
      : request<EmulatorLaunch>(`/games/${id}/launch-command`, { method: "POST" });
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
      throw new ApiError(0, "network_error", t("Cannot reach the server. Check that it is running."));
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

  /* collections */
  listCollections(): Promise<Collection[]> {
    return USING_MOCKS ? mockApi.listCollections(tokenStore.get()) : request<Collection[]>("/collections");
  },
  getCollection(id: number): Promise<Collection> {
    return USING_MOCKS ? mockApi.getCollection(tokenStore.get(), id) : request<Collection>(`/collections/${id}`);
  },
  createCollection(body: CollectionInput): Promise<Collection> {
    return USING_MOCKS
      ? mockApi.createCollection(tokenStore.get(), body)
      : request<Collection>("/collections", { method: "POST", body });
  },
  updateCollection(id: number, body: Partial<CollectionInput>): Promise<Collection> {
    return USING_MOCKS
      ? mockApi.updateCollection(tokenStore.get(), id, body)
      : request<Collection>(`/collections/${id}`, { method: "PATCH", body });
  },
  deleteCollection(id: number): Promise<void> {
    return USING_MOCKS
      ? mockApi.deleteCollection(tokenStore.get(), id)
      : request(`/collections/${id}`, { method: "DELETE" });
  },
  collectionGames(id: number, page = 1, limit = 96): Promise<GameList> {
    return USING_MOCKS
      ? mockApi.collectionGames(tokenStore.get(), id, page, limit)
      : request<GameList>(`/collections/${id}/games`, { query: { page, limit } });
  },
  addCollectionGames(id: number, gameIds: number[]): Promise<CollectionGamesResponse> {
    return USING_MOCKS
      ? mockApi.addCollectionGames(tokenStore.get(), id, gameIds)
      : request<CollectionGamesResponse>(`/collections/${id}/games`, { method: "POST", body: { gameIds } });
  },
  removeCollectionGame(id: number, gameId: number): Promise<void> {
    return USING_MOCKS
      ? mockApi.removeCollectionGame(tokenStore.get(), id, gameId)
      : request(`/collections/${id}/games/${gameId}`, { method: "DELETE" });
  },

  /* community */
  myProfile(): Promise<PublicProfile> {
    return USING_MOCKS ? mockApi.myProfile(tokenStore.get()) : request<PublicProfile>("/me/profile");
  },
  updateProfile(patch: ProfilePatch): Promise<PublicProfile> {
    return USING_MOCKS
      ? mockApi.updateProfile(tokenStore.get(), patch)
      : request<PublicProfile>("/me/profile", { method: "PATCH", body: patch });
  },
  communityFeed(limit = 20, before?: number): Promise<ActivityPage> {
    return USING_MOCKS
      ? mockApi.communityFeed(tokenStore.get(), limit, before)
      : request<ActivityPage>("/community/feed", { query: { limit, before } });
  },
  communityUsers(): Promise<PublicProfile[]> {
    return USING_MOCKS ? mockApi.communityUsers(tokenStore.get()) : request<PublicProfile[]>("/community/users");
  },
  communityUser(id: number): Promise<PublicProfileResponse> {
    return USING_MOCKS
      ? mockApi.communityUser(tokenStore.get(), id)
      : request<PublicProfileResponse>(`/community/users/${id}`);
  },
  communityUserActivity(id: number, limit = 20, before?: number): Promise<ActivityPage> {
    return USING_MOCKS
      ? mockApi.communityUserActivity(tokenStore.get(), id, limit, before)
      : request<ActivityPage>(`/community/users/${id}/activity`, { query: { limit, before } });
  },
  reportInstalled(id: number): Promise<{ recorded: boolean }> {
    return USING_MOCKS
      ? mockApi.reportInstalled(tokenStore.get(), id)
      : request<{ recorded: boolean }>(`/games/${id}/installed`, { method: "POST" });
  },

  /* achievements */
  gameAchievements(id: number): Promise<GameAchievements> {
    return USING_MOCKS
      ? mockApi.gameAchievements(tokenStore.get(), id)
      : request<GameAchievements>(`/games/${id}/achievements`);
  },
  scanAchievements(
    id: number,
    body: { installPath?: string | null; appId?: string | null; providers?: string[] } = {},
  ): Promise<AchievementScanReport> {
    return USING_MOCKS
      ? mockApi.scanAchievements(tokenStore.get(), id, body)
      : request<AchievementScanReport>(`/games/${id}/achievements/scan`, { method: "POST", body });
  },
  meAchievements(): Promise<UserAchievements> {
    return USING_MOCKS ? mockApi.meAchievements(tokenStore.get()) : request<UserAchievements>("/me/achievements");
  },
  userAchievements(id: number): Promise<UserAchievements> {
    return USING_MOCKS
      ? mockApi.userAchievements(tokenStore.get(), id)
      : request<UserAchievements>(`/users/${id}/achievements`);
  },


  /* sessions */
  listSessions(): Promise<AuthSession[]> {
    return USING_MOCKS ? mockApi.listSessions(tokenStore.get()) : request<AuthSession[]>("/auth/sessions");
  },
  revokeSession(id: number): Promise<void> {
    return USING_MOCKS
      ? mockApi.revokeSession(tokenStore.get(), id)
      : request(`/auth/sessions/${id}`, { method: "DELETE" });
  },

  /* resumable uploads */
  initUpload(body: { fileName: string; size: number; sha256?: string | null }): Promise<UploadSession> {
    return USING_MOCKS
      ? mockApi.initUpload(tokenStore.get(), body)
      : request<UploadSession>("/uploads", { method: "POST", body });
  },
  getUpload(id: string): Promise<UploadSession> {
    return USING_MOCKS
      ? mockApi.getUpload(tokenStore.get(), id)
      : request<UploadSession>(`/uploads/${encodeURIComponent(id)}`);
  },
  uploadPart(
    id: string,
    part: number,
    chunk: Blob,
    onProgress?: (loaded: number) => void,
  ): Promise<UploadPartResponse> {
    if (USING_MOCKS) return mockApi.uploadPart(tokenStore.get(), id, part, chunk, onProgress);
    return putChunk<UploadPartResponse>(`/uploads/${encodeURIComponent(id)}/parts/${part}`, chunk, onProgress);
  },
  completeUpload(id: string): Promise<CompleteUploadResponse> {
    return USING_MOCKS
      ? mockApi.completeUpload(tokenStore.get(), id)
      : request<CompleteUploadResponse>(`/uploads/${encodeURIComponent(id)}/complete`, { method: "POST" });
  },
  cancelUpload(id: string): Promise<void> {
    return USING_MOCKS
      ? mockApi.cancelUpload(tokenStore.get(), id)
      : request(`/uploads/${encodeURIComponent(id)}`, { method: "DELETE" });
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
  if (status === 401) return t("Your session expired. Sign in again.");
  if (status === 404) return t("That save no longer exists.");
  if (status === 413) return t("That save is larger than the server allows (2 GiB).");
  return t("The server refused the request ({status}).", { status });
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
 * Raw binary chunk upload. Uses XHR for progress and to keep the body
 * untouched (a fetch Blob body would otherwise be copied for large chunks).
 */
function putChunk<T>(
  path: string,
  chunk: Blob,
  onProgress?: (loaded: number) => void,
): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    const xhr = new XMLHttpRequest();
    xhr.open("PUT", buildUrl(path));
    xhr.setRequestHeader("Content-Type", "application/octet-stream");
    const token = tokenStore.get();
    if (token) xhr.setRequestHeader("Authorization", `Bearer ${token}`);
    xhr.upload.onprogress = (event) => {
      if (event.lengthComputable) onProgress?.(event.loaded);
    };
    xhr.onerror = () => reject(new ApiError(0, "network_error", t("Cannot reach the server. Check that it is running.")));
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
          envelope?.error?.message ?? `The server refused the chunk (${xhr.status}).`,
        ),
      );
    };
    xhr.send(chunk);
  });
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

    xhr.onerror = () => reject(new ApiError(0, "network_error", t("Cannot reach the server. Check that it is running.")));
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
