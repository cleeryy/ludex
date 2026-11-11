import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  ACTIVE_SERVER_KEY,
  LEGACY_TOKEN_KEY,
  activeServerId,
  activeStoredServer,
  activeTokenKey,
  ensureServerProfiles,
  localServerList,
  parseServerUrl,
  readServers,
  removeLocalServer,
  saveLocalServer,
  scopedStorageKey,
  serverIdFor,
  serverLabelFor,
  tokenKeyFor,
} from "@/lib/server-storage";
type ServersModule = typeof import("@/lib/servers");
let serversModule: ServersModule | null = null;

async function loadServers(): Promise<ServersModule> {
  if (!serversModule) serversModule = await import("@/lib/servers");
  return serversModule;
}

class MemoryStorage implements Storage {
  private map = new Map<string, string>();
  get length() {
    return this.map.size;
  }
  clear() {
    this.map.clear();
  }
  getItem(key: string) {
    return this.map.get(key) ?? null;
  }
  key(index: number) {
    return [...this.map.keys()][index] ?? null;
  }
  removeItem(key: string) {
    this.map.delete(key);
  }
  setItem(key: string, value: string) {
    this.map.set(key, value);
  }
}

let memory: MemoryStorage;

beforeEach(() => {
  memory = new MemoryStorage();
  vi.stubGlobal("localStorage", memory);
  vi.stubGlobal("window", { location: { origin: "https://ludex.example.com" }, localStorage: memory });
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("server address validation", () => {
  it("normalises the scheme, the authority and the trailing slash", () => {
    expect(parseServerUrl("  HTTPS://Ludex.Example.com/  ")).toEqual({
      ok: true,
      value: "https://ludex.example.com",
    });
    expect(parseServerUrl("ludex.example.com:8443")).toEqual({
      ok: true,
      value: "http://ludex.example.com:8443",
    });
    expect(parseServerUrl("http://example.com/base/path/")).toEqual({
      ok: true,
      value: "http://example.com/base/path",
    });
  });

  it("reports every invalid shape with a specific problem", () => {
    expect(parseServerUrl("")).toEqual({ ok: false, problem: "empty" });
    expect(parseServerUrl("ftp://example.com")).toEqual({ ok: false, problem: "scheme" });
    expect(parseServerUrl("http://user:pass@example.com")).toEqual({ ok: false, problem: "credentials" });
    expect(parseServerUrl("http://exa mple.com")).toEqual({ ok: false, problem: "whitespace" });
    expect(parseServerUrl("http://example.com/?a=1")).toEqual({ ok: false, problem: "query" });
    expect(parseServerUrl("http://:8080")).toEqual({ ok: false, problem: "port" });
  });

  it("derives a stable id and a readable label", () => {
    const id = serverIdFor("https://ludex.example.com");
    expect(id).toBe(serverIdFor("HTTPS://LUDEX.EXAMPLE.COM/"));
    expect(id).not.toBe(serverIdFor("https://ludex.example.com:8443"));
    expect(id).toMatch(/^srv_[0-9a-f]{8}$/);
    expect(serverLabelFor("https://ludex.example.com")).toBe("ludex.example.com");
  });
});

describe("single-server migration", () => {
  it("turns the legacy token into the first profile", () => {
    memory.setItem(LEGACY_TOKEN_KEY, "legacy-jwt");
    ensureServerProfiles();

    const servers = readServers();
    expect(servers).toHaveLength(1);
    expect(servers[0].url).toBe("https://ludex.example.com");
    expect(servers[0].label).toBe("ludex.example.com");
    expect(activeServerId()).toBe(servers[0].id);
    expect(memory.getItem(tokenKeyFor(servers[0].id))).toBe("legacy-jwt");
    // The legacy key stays as a fallback for other tabs and old bookmarks.
    expect(memory.getItem(LEGACY_TOKEN_KEY)).toBe("legacy-jwt");

    ensureServerProfiles();
    expect(readServers()).toHaveLength(1);
  });

  it("tracks the current origin as the first profile even before a login", () => {
    ensureServerProfiles();
    const servers = readServers();
    expect(servers).toHaveLength(1);
    expect(servers[0].url).toBe("https://ludex.example.com");
    expect(activeServerId()).toBe(servers[0].id);
  });
});

describe("per-server isolation", () => {
  it("scopes tokens and UI keys to the active server", () => {
    const one = saveLocalServer({ label: "One", url: "https://one.example.com" });
    const two = saveLocalServer({ label: "Two", url: "https://two.example.com" });
    expect(one.servers).toHaveLength(1);

    const first = activeServerId();
    expect(first).toBeTruthy();
    memory.setItem(tokenKeyFor(first!), "jwt-one");
    expect(activeTokenKey()).toBe(tokenKeyFor(first!));
    expect(scopedStorageKey("ludex.library.filters")).toBe(`ludex.server.${first}.ludex.library.filters`);

    const second = two.servers.find((server) => server.id !== first);
    expect(second).toBeTruthy();
    expect(second!.id).not.toBe(first);
    // Saving the second server does not steal the active slot.
    expect(activeServerId()).toBe(first);
    expect(activeStoredServer()?.label).toBe("One");
  });

  it("refuses a duplicate URL", () => {
    saveLocalServer({ label: "One", url: "https://one.example.com" });
    expect(() => saveLocalServer({ label: "Duplicate", url: "https://one.example.com/" })).toThrow(
      "already_saved",
    );
  });

  it("removes the token and falls back to the first remaining server", () => {
    saveLocalServer({ label: "One", url: "https://one.example.com" });
    const one = activeServerId()!;
    memory.setItem(tokenKeyFor(one), "jwt-one");
    const list = saveLocalServer({ label: "Two", url: "https://two.example.com" });
    const two = list.servers.find((server) => server.id !== one)!.id;

    // Activate the second one, then remove it: the first becomes active.
    memory.setItem(ACTIVE_SERVER_KEY, two);
    const after = removeLocalServer(two);
    expect(after.activeId).toBe(one);
    expect(after.servers).toHaveLength(1);
    expect(memory.getItem(tokenKeyFor(two))).toBeNull();
    expect(memory.getItem(tokenKeyFor(one))).toBe("jwt-one");
  });

  it("exposes the saved tokens to the list", () => {
    const list = saveLocalServer({ label: "One", url: "https://one.example.com", token: "jwt" });
    expect(list.servers[0].hasToken).toBe(true);
    expect(localServerList().servers[0].active).toBe(true);
  });
});

describe("connection test", () => {
  it("reports a healthy server with its version", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => new Response(JSON.stringify({ status: "ok", version: "9.9.9" }), { status: 200 })),
    );
    const { testServer } = await loadServers();
    const result = await testServer("https://ludex.example.com");
    expect(result.ok).toBe(true);
    expect(result.version).toBe("9.9.9");
    expect(result.status).toBe(200);
  });

  it("reports an unreachable server honestly", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => {
        throw new Error("connection refused");
      }),
    );
    const { testServer } = await loadServers();
    const result = await testServer("https://ludex.example.com");
    expect(result.ok).toBe(false);
    expect(result.message).toContain("Could not reach");
  });

  it("rejects an invalid address without a request", async () => {
    const fetchMock = vi.fn();
    vi.stubGlobal("fetch", fetchMock);
    const { testServer } = await loadServers();
    const result = await testServer("not a url");
    expect(result.ok).toBe(false);
    expect(fetchMock).not.toHaveBeenCalled();
  });
});
