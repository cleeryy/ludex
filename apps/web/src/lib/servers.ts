import { setApiBase, tokenStore } from "@/lib/api";
import { isNative, native } from "@/lib/native";
import {
  activeStoredServer,
  ensureServerProfiles,
  localServerList,
  parseServerUrl,
  readServers,
  removeLocalServer,
  saveLocalServer,
  setActiveServerId,
  tokenKeyFor,
} from "@/lib/server-storage";
import type { ServerList, ServerTest } from "@/lib/types";

/**
 * One surface for saved servers in the browser and the desktop app.
 *
 * Browser profiles live in localStorage (see `server-storage.ts`); the desktop
 * app keeps them in `settings.json` with tokens in the OS keychain or its SQLite
 * fallback. Both return the same shape so the switcher and the settings panel do
 * not care which build they run in.
 */

const emptyList = (message: string | null = null): ServerList => ({
  servers: [],
  activeId: null,
  tokenStorage: "database",
  keychain: null,
  warning: null,
  message,
});

export async function listServers(): Promise<ServerList> {
  if (isNative()) {
    try {
      return await native.listServers();
    } catch {
      return emptyList();
    }
  }
  ensureServerProfiles();
  return localServerList();
}

export async function saveServer(input: {
  id?: string | null;
  label: string;
  url: string;
  username?: string | null;
  token?: string | null;
}): Promise<ServerList> {
  if (isNative()) {
    return native.saveServer({
      id: input.id ?? null,
      label: input.label,
      url: input.url,
      username: input.username ?? null,
      token: input.token ?? null,
    });
  }
  return saveLocalServer(input);
}

export async function removeServer(id: string): Promise<ServerList> {
  if (isNative()) {
    return native.removeServer(id);
  }
  return removeLocalServer(id);
}

/**
 * Make `id` the active server and point the API client at it. The caller
 * re-boots the auth provider so the session comes from the new server.
 */
export async function activateServer(id: string): Promise<ServerList> {
  if (isNative()) {
    const list = await native.switchServer(id);
    const settings = await native.getSettings();
    if (settings?.serverUrl) setApiBase(settings.serverUrl);
    if (settings?.token) tokenStore.set(settings.token);
    else tokenStore.clear();
    return list;
  }

  const profile = readServers().find((server) => server.id === id);
  if (!profile) throw new Error("That server is not saved.");
  setActiveServerId(id);
  setApiBase(profile.url);
  const token = typeof localStorage === "undefined" ? null : localStorage.getItem(tokenKeyFor(id));
  if (token) tokenStore.set(token);
  else tokenStore.clear();
  return localServerList();
}

/** Browser boot: point the API client at the active saved server. */
export function bootstrapBrowserServer(): void {
  if (isNative()) return;
  ensureServerProfiles();
  const profile = activeStoredServer();
  if (!profile) return;
  setApiBase(profile.url);
  const token = typeof localStorage === "undefined" ? null : localStorage.getItem(tokenKeyFor(profile.id));
  if (token) tokenStore.set(token);
  else tokenStore.clear();
}

/** `GET {url}/api/health`, with the same result shape as the desktop command. */
export async function testServer(url: string, token?: string | null): Promise<ServerTest> {
  if (isNative()) {
    return native.testServer(url, token ?? null);
  }
  const parsed = parseServerUrl(url);
  if (!parsed.ok) {
    return {
      ok: false,
      status: null,
      version: null,
      latencyMs: 0,
      message: "That server address is not valid.",
    };
  }
  const started = Date.now();
  try {
    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), 8_000);
    const response = await fetch(`${parsed.value}/api/health`, {
      headers: token ? { Authorization: `Bearer ${token}` } : undefined,
      signal: controller.signal,
    }).finally(() => clearTimeout(timeout));
    const latencyMs = Date.now() - started;
    const payload = (await response.json().catch(() => null)) as { version?: string } | null;
    if (!response.ok) {
      return {
        ok: false,
        status: response.status,
        version: payload?.version ?? null,
        latencyMs,
        message: `The server answered with HTTP ${response.status}.`,
      };
    }
    return {
      ok: true,
      status: response.status,
      version: payload?.version ?? null,
      latencyMs,
      message: payload?.version
        ? `Connected. Ludex ${payload.version} answered in ${latencyMs} ms.`
        : `Connected in ${latencyMs} ms.`,
    };
  } catch {
    return {
      ok: false,
      status: null,
      version: null,
      latencyMs: Date.now() - started,
      message: `Could not reach ${parsed.value}.`,
    };
  }
}
