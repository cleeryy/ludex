import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

/**
 * Desktop handoff helpers (#157).
 *
 * `createDesktopHandoff` is a thin wrapper over the API client; the interesting
 * behaviour is that the state survives the OIDC redirect and that a normal web
 * login never creates a handoff.
 */
const apiMock = vi.hoisted(() => ({ createDesktopHandoff: vi.fn() }));
vi.mock("@/lib/api", () => ({ api: { createDesktopHandoff: apiMock.createDesktopHandoff } }));

import {
  createDesktopHandoff,
  forgetDesktopState,
  isValidDesktopState,
  pendingDesktopState,
  readDesktopStateFromSearch,
  rememberDesktopState,
} from "./desktop-handoff";

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

const BASE64URL_STATE = "A".repeat(43);
const HEX_STATE = "0123456789abcdef".repeat(4);

describe("desktop handoff", () => {
  let storage: MemoryStorage;

  beforeEach(() => {
    storage = new MemoryStorage();
    vi.stubGlobal("sessionStorage", storage);
    apiMock.createDesktopHandoff.mockReset();
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("accepts only high-entropy state shapes", () => {
    expect(isValidDesktopState(BASE64URL_STATE)).toBe(true);
    expect(isValidDesktopState(HEX_STATE)).toBe(true);
    expect(isValidDesktopState(`${BASE64URL_STATE}-_azAZ09`)).toBe(true);
    expect(isValidDesktopState(null)).toBe(false);
    expect(isValidDesktopState(undefined)).toBe(false);
    expect(isValidDesktopState("")).toBe(false);
    expect(isValidDesktopState("A".repeat(42))).toBe(false);
    expect(isValidDesktopState("A".repeat(129))).toBe(false);
    expect(isValidDesktopState(`${"A".repeat(42)}!`)).toBe(false);
    expect(isValidDesktopState(`${"A".repeat(42)} `)).toBe(false);
    expect(isValidDesktopState("é".repeat(43))).toBe(false);
  });

  it("reads the parameter from the hash route and ignores anything else", () => {
    expect(readDesktopStateFromSearch(`?desktop_state=${BASE64URL_STATE}`)).toBe(BASE64URL_STATE);
    expect(readDesktopStateFromSearch(`oidc_code=abc&desktop_state=${HEX_STATE}`)).toBe(HEX_STATE);
    expect(readDesktopStateFromSearch(`#desktop_state=${HEX_STATE}`)).toBe(HEX_STATE);
    expect(readDesktopStateFromSearch(`?desktop_state=short`)).toBeNull();
    expect(readDesktopStateFromSearch("?oidc_code=abc")).toBeNull();
    expect(readDesktopStateFromSearch("")).toBeNull();
    expect(readDesktopStateFromSearch(new URLSearchParams("desktop_state=short"))).toBeNull();
  });

  it("remembers the state across the OIDC redirect and forgets it afterwards", () => {
    expect(pendingDesktopState()).toBeNull();

    rememberDesktopState(HEX_STATE);
    // The provider redirect lands on `/#/login?oidc_code=…`: the URL no longer
    // carries the parameter, the tab does.
    expect(pendingDesktopState()).toBe(HEX_STATE);

    forgetDesktopState();
    expect(pendingDesktopState()).toBeNull();
  });

  it("ignores a corrupted stored value", () => {
    storage.setItem("ludex.desktop-state", "garbage");
    expect(pendingDesktopState()).toBeNull();
  });

  it("survives a storage that throws (private mode)", () => {
    vi.stubGlobal("sessionStorage", {
      getItem() {
        throw new Error("denied");
      },
      setItem() {
        throw new Error("denied");
      },
      removeItem() {
        throw new Error("denied");
      },
    });
    expect(pendingDesktopState()).toBeNull();
    expect(() => rememberDesktopState(HEX_STATE)).not.toThrow();
    expect(() => forgetDesktopState()).not.toThrow();
  });

  it("creates a handoff through the authenticated API only when asked", async () => {
    apiMock.createDesktopHandoff.mockResolvedValue({ expiresAt: "2026-11-11T10:02:00Z" });
    const result = await createDesktopHandoff(HEX_STATE);
    expect(result.expiresAt).toBe("2026-11-11T10:02:00Z");
    expect(apiMock.createDesktopHandoff).toHaveBeenCalledTimes(1);
    expect(apiMock.createDesktopHandoff).toHaveBeenCalledWith(HEX_STATE);
  });

  it("surfaces a rejected handoff so the page can offer a retry", async () => {
    apiMock.createDesktopHandoff.mockRejectedValue(
      new Error("This sign-in handoff is unknown, expired or already used."),
    );
    await expect(createDesktopHandoff(HEX_STATE)).rejects.toThrow("already used");
  });
});
