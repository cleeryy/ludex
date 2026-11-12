import { describe, expect, it } from "vitest";
import { createPrefsStore } from "./use-controller-prefs";
import type { ControllerStorage } from "./controller";

/* --------------------------------------------------------------------------
   #198: one device store for the profile, haptics and sound. The store is the
   contract between the console screen and Settings → Appearance, so the tests
   cover persistence, sanitising and the subscription.
   -------------------------------------------------------------------------- */

function memoryStorage(entries: Record<string, string> = {}) {
  const map = new Map(Object.entries(entries));
  const storage: ControllerStorage & { map: Map<string, string> } = {
    map,
    getItem: (key) => map.get(key) ?? null,
    setItem: (key, value) => {
      map.set(key, value);
    },
  };
  return storage;
}

describe("Big Picture prefs store", () => {
  it("persists a patch and notifies subscribers", () => {
    const storage = memoryStorage();
    const store = createPrefsStore({ storage, key: "test.prefs", listen: false });
    let notified = 0;
    const off = store.subscribe(() => {
      notified += 1;
    });

    store.update({ haptics: false });
    expect(store.get().haptics).toBe(false);
    expect(JSON.parse(storage.map.get("test.prefs") ?? "null")).toMatchObject({ haptics: false });

    store.update({ sound: { enabled: false, volume: 0.2 } });
    expect(store.get().sound).toEqual({ enabled: false, volume: 0.2 });
    expect(notified).toBe(2);

    off();
    store.update({ profileId: "account" });
    expect(notified).toBe(2);
  });

  it("reads an existing record and fills the sound and haptics fields", () => {
    const storage = memoryStorage({
      "test.prefs": JSON.stringify({ profileId: "account", haptics: false }),
    });
    const store = createPrefsStore({ storage, key: "test.prefs", listen: false });
    expect(store.get()).toEqual({
      profileId: "account",
      haptics: false,
      hapticsStrength: "standard",
      sound: { enabled: true, volume: 0.45 },
    });
  });

  it("sanitises on write, so hand-edited values never reach the screen", () => {
    const storage = memoryStorage();
    const store = createPrefsStore({ storage, key: "test.prefs", listen: false });
    store.set({
      profileId: "  ",
      haptics: true,
      hapticsStrength: "loud" as never,
      sound: { enabled: true, volume: 9 },
    });
    expect(store.get()).toEqual({
      profileId: null,
      haptics: true,
      hapticsStrength: "standard",
      sound: { enabled: true, volume: 1 },
    });
    expect(JSON.parse(storage.map.get("test.prefs") ?? "null")).toMatchObject({ sound: { volume: 1 } });
  });

  it("survives storage that throws", () => {
    const blocked: ControllerStorage = {
      getItem: () => {
        throw new Error("SecurityError");
      },
      setItem: () => {
        throw new Error("QuotaExceededError");
      },
    };
    const store = createPrefsStore({ storage: blocked, key: "test.prefs", listen: false });
    expect(store.get().haptics).toBe(true);
    expect(store.update({ haptics: false }).haptics).toBe(false);
    expect(store.get().haptics).toBe(false);
  });
});
