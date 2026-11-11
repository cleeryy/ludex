import type { ServerList, ServerProfile } from "@/lib/types";

/**
 * Browser-side server profiles (#24).
 *
 * The desktop app keeps profiles in `settings.json` and tokens in the OS
 * keychain (or its SQLite fallback); this module is the browser equivalent:
 * metadata in one localStorage record, one token key per server, and one active
 * server. Existing single-server installs migrate on first load: the current
 * token becomes the active profile's token and the legacy key stays as a
 * fallback.
 */

export const SERVERS_KEY = "ludex.servers.v1";
export const ACTIVE_SERVER_KEY = "ludex.activeServer.v1";
export const LEGACY_TOKEN_KEY = "ludex.token";
export const legacyTokenKey = LEGACY_TOKEN_KEY;

export interface StoredServer {
  id: string;
  label: string;
  url: string;
  username: string | null;
  lastUsedAt: string | null;
  createdAt: string | null;
}

export type ServerUrlProblem =
  | "empty"
  | "scheme"
  | "host"
  | "credentials"
  | "whitespace"
  | "query"
  | "port";

export type ParsedServerUrl =
  | { ok: true; value: string }
  | { ok: false; problem: ServerUrlProblem };

export function isTauriShell(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in (window as unknown as Record<string, unknown>);
}

function storage(): Storage | null {
  if (typeof localStorage === "undefined") return null;
  try {
    return localStorage;
  } catch {
    return null;
  }
}

/** Add `http://` when missing, lowercase the scheme and authority, clean the path. */
export function parseServerUrl(input: string): ParsedServerUrl {
  const trimmed = input.trim();
  if (!trimmed) return { ok: false, problem: "empty" };
  const candidate = trimmed.includes("://") ? trimmed : `http://${trimmed}`;
  const split = candidate.indexOf("://");
  if (split === -1) return { ok: false, problem: "scheme" };
  const scheme = candidate.slice(0, split).toLowerCase();
  if (scheme !== "http" && scheme !== "https") return { ok: false, problem: "scheme" };

  const rest = candidate.slice(split + 3);
  if (!rest) return { ok: false, problem: "host" };
  if (rest.includes("@")) return { ok: false, problem: "credentials" };
  if (/\s/.test(rest)) return { ok: false, problem: "whitespace" };

  const slash = rest.indexOf("/");
  const authority = slash === -1 ? rest : rest.slice(0, slash);
  const path = slash === -1 ? "" : rest.slice(slash + 1);
  if (!authority) return { ok: false, problem: "host" };
  if (authority.includes("?") || authority.includes("#") || path.includes("?") || path.includes("#")) {
    return { ok: false, problem: "query" };
  }
  if (authority.startsWith(":") || authority.endsWith(":")) return { ok: false, problem: "port" };

  const cleanPath = path.replace(/^\/+|\/+$/g, "");
  const value = cleanPath ? `${scheme}://${authority.toLowerCase()}/${cleanPath}` : `${scheme}://${authority.toLowerCase()}`;
  return { ok: true, value };
}

/** Throws when the address is invalid; use `parseServerUrl` for UI messages. */
export function normalizeServerUrl(input: string): string {
  const parsed = parseServerUrl(input);
  if (!parsed.ok) throw new Error(`invalid_server_url:${parsed.problem}`);
  return parsed.value;
}

/** Stable profile id derived from the normalised URL (FNV-1a, 32 bit). */
export function serverIdFor(url: string): string {
  const normalized = normalizeServerUrl(url);
  let hash = 0x811c9dc5;
  for (const character of normalized) {
    hash ^= character.codePointAt(0) ?? 0;
    hash = Math.imul(hash, 0x01000193) >>> 0;
  }
  return `srv_${hash.toString(16).padStart(8, "0")}`;
}

export function serverLabelFor(url: string): string {
  const parsed = parseServerUrl(url);
  if (!parsed.ok) return url;
  return parsed.value.split("://")[1]?.split("/")[0] ?? parsed.value;
}

export function tokenKeyFor(serverId: string): string {
  return `ludex.token.${serverId}`;
}

export function readServers(): StoredServer[] {
  const store = storage();
  if (!store) return [];
  try {
    const parsed: unknown = JSON.parse(store.getItem(SERVERS_KEY) ?? "[]");
    if (!Array.isArray(parsed)) return [];
    return parsed.flatMap((entry) => {
      if (!entry || typeof entry !== "object") return [];
      const record = entry as Record<string, unknown>;
      if (typeof record.id !== "string" || typeof record.url !== "string") return [];
      const parsedUrl = parseServerUrl(record.url);
      if (!parsedUrl.ok) return [];
      return [
        {
          id: record.id,
          label: typeof record.label === "string" && record.label.trim() ? record.label.trim() : serverLabelFor(parsedUrl.value),
          url: parsedUrl.value,
          username: typeof record.username === "string" && record.username.trim() ? record.username.trim() : null,
          lastUsedAt: typeof record.lastUsedAt === "string" ? record.lastUsedAt : null,
          createdAt: typeof record.createdAt === "string" ? record.createdAt : null,
        },
      ];
    });
  } catch {
    return [];
  }
}

export function writeServers(servers: StoredServer[]): void {
  const store = storage();
  if (!store) return;
  store.setItem(SERVERS_KEY, JSON.stringify(servers));
}

export function activeServerId(): string | null {
  const store = storage();
  if (!store) return null;
  const value = store.getItem(ACTIVE_SERVER_KEY);
  return value && readServers().some((server) => server.id === value) ? value : null;
}

export function setActiveServerId(id: string | null): void {
  const store = storage();
  if (!store) return;
  if (id) store.setItem(ACTIVE_SERVER_KEY, id);
  else store.removeItem(ACTIVE_SERVER_KEY);
}

export function activeStoredServer(): StoredServer | null {
  const id = activeServerId();
  if (!id) return null;
  return readServers().find((server) => server.id === id) ?? null;
}

export function activeTokenKey(): string | null {
  const id = activeServerId();
  return id ? tokenKeyFor(id) : null;
}

/**
 * `null` in the desktop shell (the token comes from settings.json) or when no
 * server is saved. Otherwise the browser's active per-server token scope.
 */
export function activeStorageScope(): { serverId: string; tokenKey: string } | null {
  if (isTauriShell()) return null;
  const id = activeServerId();
  return id ? { serverId: id, tokenKey: tokenKeyFor(id) } : null;
}

/**
 * Namespaced localStorage key for per-server UI state. The desktop app keeps
 * its state in SQLite per profile, so keys stay unchanged there.
 */
export function scopedStorageKey(name: string): string {
  if (isTauriShell()) return name;
  const active = activeStoredServer();
  return active ? `ludex.server.${active.id}.${name}` : name;
}

/**
 * Migrate the single-server setup: a saved token implies the current origin is
 * the server. Idempotent and a no-op without localStorage.
 */
export function ensureServerProfiles(): void {
  if (isTauriShell()) return;
  const store = storage();
  if (!store) return;
  let servers = readServers();

  if (servers.length === 0) {
    const envUrl = (import.meta.env.VITE_API_URL as string | undefined) ?? "";
    const fallbackUrl = envUrl || (typeof window !== "undefined" ? window.location.origin : "");
    const parsed = fallbackUrl ? parseServerUrl(fallbackUrl) : ({ ok: false, problem: "empty" } as const);
    if (parsed.ok) {
      const now = new Date().toISOString();
      const profile: StoredServer = {
        id: serverIdFor(parsed.value),
        label: serverLabelFor(parsed.value),
        url: parsed.value,
        username: null,
        lastUsedAt: now,
        createdAt: now,
      };
      servers = [profile];
      writeServers(servers);
      setActiveServerId(profile.id);
      const legacyToken = store.getItem(LEGACY_TOKEN_KEY);
      if (legacyToken) store.setItem(tokenKeyFor(profile.id), legacyToken);
    }
  }

  if (servers.length > 0 && !activeServerId()) {
    setActiveServerId(servers[0].id);
  }
}

/** Server list in the shape the desktop commands return. */
export function localServerList(message: string | null = null): ServerList {
  const servers = readServers();
  const active = activeServerId();
  const store = storage();
  const views: ServerProfile[] = servers.map((server) => ({
    id: server.id,
    label: server.label,
    url: server.url,
    username: server.username,
    lastUsedAt: server.lastUsedAt,
    createdAt: server.createdAt,
    active: server.id === active,
    local: false,
    hasToken: Boolean(store?.getItem(tokenKeyFor(server.id))),
  }));
  return {
    servers: views,
    activeId: active,
    tokenStorage: "database",
    keychain: null,
    warning: null,
    message,
  };
}

export function saveLocalServer(input: {
  id?: string | null;
  label: string;
  url: string;
  username?: string | null;
  token?: string | null;
}): ServerList {
  const parsed = parseServerUrl(input.url);
  if (!parsed.ok) throw new Error(`invalid_server_url:${parsed.problem}`);
  const url = parsed.value;
  const servers = readServers();
  const existing = input.id ? servers.find((server) => server.id === input.id) : undefined;
  const id = serverIdFor(url);

  if (servers.some((server) => server.id === id && server.id !== existing?.id)) {
    throw new Error("already_saved");
  }

  const now = new Date().toISOString();
  const label = input.label.trim().slice(0, 60) || serverLabelFor(url);
  const username = input.username?.trim() || null;
  const next: StoredServer = {
    id,
    label,
    url,
    username: username ?? existing?.username ?? null,
    lastUsedAt: existing?.lastUsedAt ?? null,
    createdAt: existing?.createdAt ?? now,
  };

  const list = existing
    ? servers.map((server) => (server.id === existing.id ? next : server))
    : [...servers, next];

  writeServers(list);
  if (existing && activeServerId() === existing.id) {
    setActiveServerId(id);
  }
  if (!activeServerId()) setActiveServerId(id);

  const token = input.token?.trim();
  if (token) {
    storage()?.setItem(tokenKeyFor(id), token);
  }
  return localServerList();
}

export function removeLocalServer(id: string): ServerList {
  const servers = readServers();
  const wasActive = activeServerId() === id;
  const remaining = servers.filter((server) => server.id !== id);
  writeServers(remaining);
  storage()?.removeItem(tokenKeyFor(id));

  if (wasActive) {
    setActiveServerId(remaining[0]?.id ?? null);
  }
  return localServerList();
}
