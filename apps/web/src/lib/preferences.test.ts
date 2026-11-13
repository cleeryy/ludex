import { describe, expect, it } from "vitest";
import { BIG_PICTURE_LAYOUT_KEY } from "@/components/big-picture/layout";
import { customThemeId } from "@/theme/storage";
import {
  LIBRARY_VIEW_STORAGE_KEY,
  PREFERENCES_CACHE_PREFIX,
  SOUND_STORAGE_KEY,
  UPDATE_NOTIFICATIONS_STORAGE_KEY,
  UPDATE_PROMPT_STORAGE_KEY,
  defaultPreferences,
  mergePreferences,
  preferencePatch,
  preferencesCacheKey,
  preferencesEqual,
  readCachedPreferences,
  readLocalPreferences,
  sanitizePreferences,
  themeAvailable,
  writeCachedPreferences,
} from "./preferences";

function memoryStorage(initial: Record<string, string> = {}) {
  const entries = new Map(Object.entries(initial));
  return {
    entries,
    getItem: (key: string) => entries.get(key) ?? null,
    setItem: (key: string, value: string) => {
      entries.set(key, value);
    },
    removeItem: (key: string) => {
      entries.delete(key);
    },
  };
}

describe("preference document", () => {
  it("defaults are complete and only carry account-scoped keys", () => {
    const document = defaultPreferences();
    expect(Object.keys(document).sort()).toEqual(["bigPicture", "library", "theme", "updates"]);
    expect(document.bigPicture.haptics).toBe(true);
    expect(document.library.view).toBe("grid");
    expect(document.updates.prompt).toBe("always");
    expect(document.updates.notifications).toBe("app-and-os");
    // Device-scoped values have no place in the document.
    const raw = JSON.stringify(document);
    for (const key of ["install", "download", "bandwidth", "sunshine", "moonlight", "token", "serverUrl"]) {
      expect(raw.toLowerCase()).not.toContain(key.toLowerCase());
    }
  });

  it("sanitises unknown keys, bad types and colours", () => {
    const document = sanitizePreferences({
      theme: { selection: "light", accent: "#AABBCC", tint: "not-a-colour" },
      bigPicture: { layout: "cinema", sound: "yes", haptics: false },
      library: { view: "list" },
      updates: { prompt: "sometimes", notifications: "os-only" },
      installRoot: "D:/Games",
    });
    expect(document.theme.selection).toBe("light");
    expect(document.theme.accent).toBe("#aabbcc");
    expect(document.theme.tint).toBeNull();
    expect(document.bigPicture.layout).toBe("shelf");
    expect(document.bigPicture.sound).toBe(true);
    expect(document.bigPicture.haptics).toBe(false);
    expect(document.library.view).toBe("list");
    expect(document.updates.prompt).toBe("always");
    expect(document.updates.notifications).toBe("app-and-os");
    expect("installRoot" in document).toBe(false);
  });

  it("falls back to the defaults for a missing or malformed document", () => {
    expect(sanitizePreferences(null)).toEqual(defaultPreferences());
    expect(sanitizePreferences("midnight")).toEqual(defaultPreferences());
    expect(sanitizePreferences([1, 2])).toEqual(defaultPreferences());
  });

  it("merges partial patches without dropping sibling keys", () => {
    const base = defaultPreferences();
    const merged = mergePreferences(base, {
      theme: { accent: "#112233" },
      library: { view: "list" },
    });
    expect(merged.theme.selection).toBe(base.theme.selection);
    expect(merged.theme.accent).toBe("#112233");
    expect(merged.library.view).toBe("list");
    expect(merged.bigPicture).toEqual(base.bigPicture);
  });

  it("diffs only the keys that changed", () => {
    const from = defaultPreferences();
    const to = mergePreferences(from, {
      bigPicture: { layout: "grid" },
      updates: { prompt: "off" },
    });
    expect(preferencePatch(from, to)).toEqual({
      bigPicture: { layout: "grid", sound: true, haptics: true },
      updates: { prompt: "off", notifications: "app-and-os" },
    });
    expect(preferencePatch(from, from)).toEqual({});
    expect(preferencesEqual(from, to)).toBe(false);
    expect(preferencesEqual(from, defaultPreferences())).toBe(true);
  });

  it("keeps custom theme selections only where the file exists", () => {
    const tokens = { bg: "#000000" } as never;
    const custom = { name: "Mara", tokens } as never;
    const id = customThemeId("Mara", tokens);
    const document = sanitizePreferences({ theme: { selection: id } });
    expect(document.theme.selection).toBe(id);
    expect(themeAvailable(id, [custom])).toBe(true);
    expect(themeAvailable(id, [])).toBe(false);
    expect(themeAvailable("midnight", [])).toBe(true);
  });
});

describe("update notification preference (#213)", () => {
  it("keeps the three channels and rejects anything else", () => {
    for (const value of ["app-and-os", "app", "off"] as const) {
      expect(sanitizePreferences({ updates: { notifications: value } }).updates.notifications).toBe(value);
    }
    expect(sanitizePreferences({ updates: { notifications: "os" } }).updates.notifications).toBe(
      "app-and-os",
    );
    expect(sanitizePreferences({ updates: { notifications: 7 } }).updates.notifications).toBe(
      "app-and-os",
    );
  });

  it("merges and diffs the channel without touching the prompt", () => {
    const base = defaultPreferences();
    const merged = mergePreferences(base, { updates: { notifications: "off" } });
    expect(merged.updates).toEqual({ prompt: "always", notifications: "off" });
    expect(preferencePatch(base, merged)).toEqual({
      updates: { prompt: "always", notifications: "off" },
    });
    expect(preferencesEqual(base, mergePreferences(base, {}))).toBe(true);
  });
});

describe("preference cache", () => {
  it("round-trips a cached document with its revision and pending flag", () => {
    const storage = memoryStorage();
    writeCachedPreferences(
      7,
      { document: mergePreferences(defaultPreferences(), { library: { view: "list" } }), revision: 4, pending: true },
      storage,
    );
    const cached = readCachedPreferences(7, storage);
    expect(cached?.revision).toBe(4);
    expect(cached?.pending).toBe(true);
    expect(cached?.document.library.view).toBe("list");
    expect(preferencesCacheKey(7)).toContain(PREFERENCES_CACHE_PREFIX);
  });

  it("scopes the cache per account and ignores corrupt records", () => {
    const storage = memoryStorage({
      [preferencesCacheKey(1)]: JSON.stringify({
        document: { library: { view: "list" } },
        revision: 2,
        pending: false,
      }),
      [preferencesCacheKey(2)]: "{not json",
    });
    expect(readCachedPreferences(1, storage)?.document.library.view).toBe("list");
    expect(readCachedPreferences(2, storage)).toBeNull();
    expect(readCachedPreferences(3, storage)).toBeNull();
  });
});

describe("local snapshot", () => {
  it("reads the account-scoped values this device already stores", () => {
    const storage = memoryStorage({
      "ludex.theme": JSON.stringify({
        config: { selection: "light", accent: "#223344", tint: null },
      }),
      [BIG_PICTURE_LAYOUT_KEY]: "compact",
      [LIBRARY_VIEW_STORAGE_KEY]: "list",
      "ludex.bigPicture.controller": JSON.stringify({ profileId: "pad-1", haptics: false }),
      [SOUND_STORAGE_KEY]: "off",
      [UPDATE_PROMPT_STORAGE_KEY]: "notify",
      [UPDATE_NOTIFICATIONS_STORAGE_KEY]: "app",
    });
    const local = readLocalPreferences(storage);
    expect(local.theme.selection).toBe("light");
    expect(local.theme.accent).toBe("#223344");
    expect(local.bigPicture.layout).toBe("compact");
    expect(local.bigPicture.sound).toBe(false);
    expect(local.bigPicture.haptics).toBe(false);
    expect(local.library.view).toBe("list");
    expect(local.updates.prompt).toBe("notify");
    expect(local.updates.notifications).toBe("app");
    // The controller hardware mapping stays device-local, out of the document.
    expect(JSON.stringify(local)).not.toContain("pad-1");
  });

  it("falls back to the defaults without storage", () => {
    expect(readLocalPreferences(null)).toEqual(defaultPreferences());
  });
});
